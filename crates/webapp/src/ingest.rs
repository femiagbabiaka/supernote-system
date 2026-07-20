//! Ingest API used by the ingest agent:
//! - `POST /api/pages/check` — dedup probe against the page_state ledger.
//! - `POST /api/ingest` — multipart page image + metadata → transcription.
//!
//! Meetings are created here, from the page itself: the template background
//! identifies the series (`s<id>_*.png`), and the handwritten "Meeting:" /
//! "With:" header lines supply the title and attendees. No calendar.

use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use supernote_core::grammar;
use supernote_core::models::{Area, MeetingSeries, Person};
use supernote_core::template_spec::TemplateSpec;

use crate::claude::Transcribed;
use crate::state::{internal_error, AppState};

#[derive(Debug, Deserialize)]
pub struct PageCheck {
    pub note_path: String,
    pub page_index: i64,
    pub ink_hash: String,
}

#[derive(Debug, Serialize)]
pub struct PageCheckResponse {
    /// True if this (note, page) + hash has not been ingested yet.
    pub new: bool,
}

pub async fn check_page(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PageCheck>,
) -> Result<Json<PageCheckResponse>, (StatusCode, String)> {
    let existing: Option<(String,)> = sqlx::query_as(
        "SELECT ink_hash FROM page_state WHERE note_path = ? AND page_index = ?",
    )
    .bind(&req.note_path)
    .bind(req.page_index)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| internal_error(e.into()))?;
    let new = existing.map(|(h,)| h != req.ink_hash).unwrap_or(true);
    Ok(Json(PageCheckResponse { new }))
}

#[derive(Debug, Serialize)]
pub struct IngestResponse {
    pub transcription_id: i64,
    pub meeting_id: Option<i64>,
}

pub async fn ingest(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<IngestResponse>, (StatusCode, String)> {
    let mut note_path = None;
    let mut page_index: Option<i64> = None;
    let mut ink_hash = None;
    let mut template_name: Option<String> = None;
    let mut image: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
    {
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "note_path" => note_path = Some(field.text().await.unwrap_or_default()),
            "page_index" => page_index = field.text().await.ok().and_then(|s| s.parse().ok()),
            "ink_hash" => ink_hash = Some(field.text().await.unwrap_or_default()),
            "template" => template_name = field.text().await.ok().filter(|s| !s.is_empty()),
            "image" => {
                image = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
                        .to_vec(),
                )
            }
            _ => {}
        }
    }

    let note_path = note_path.ok_or((StatusCode::BAD_REQUEST, "note_path required".into()))?;
    let page_index = page_index.ok_or((StatusCode::BAD_REQUEST, "page_index required".into()))?;
    let ink_hash = ink_hash.ok_or((StatusCode::BAD_REQUEST, "ink_hash required".into()))?;
    let image = image.ok_or((StatusCode::BAD_REQUEST, "image required".into()))?;

    process_page(&state, &note_path, page_index, &ink_hash, template_name, image)
        .await
        .map(Json)
        .map_err(internal_error)
}

/// What kind of page a template background identifies.
#[derive(Debug, PartialEq)]
enum PageKind {
    /// A standing-meeting template: `s<id>_<slug>`.
    Series(i64),
    /// The reading/listening template.
    Reading,
    /// The generic ad-hoc meeting template, or anything unrecognized.
    AdHoc,
}

/// Classify a template name like `user_s3_1-1-priya` / `reading.png`.
fn classify_template(name: Option<&str>) -> PageKind {
    let Some(name) = name else {
        return PageKind::AdHoc;
    };
    let stem = name.strip_prefix("user_").unwrap_or(name);
    let stem = stem.strip_suffix(".png").unwrap_or(stem);
    if stem == "reading" {
        return PageKind::Reading;
    }
    let series = || -> Option<i64> {
        let rest = stem.strip_prefix('s')?;
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() || !rest[digits.len()..].starts_with('_') {
            return None;
        }
        digits.parse().ok()
    };
    series().map(PageKind::Series).unwrap_or(PageKind::AdHoc)
}

/// Meeting date: `YYYYMMDD` prefix of the daily notebook's file name when
/// present (e.g. `Note/20260720.note`), otherwise today.
fn meeting_date(note_path: &str) -> NaiveDate {
    std::path::Path::new(note_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| {
            let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.len() >= 8 {
                NaiveDate::parse_from_str(&digits[..8], "%Y%m%d").ok()
            } else {
                None
            }
        })
        .unwrap_or_else(|| Utc::now().date_naive())
}

fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .take(5)
        .collect::<Vec<_>>()
        .join("-")
}

async fn process_page(
    state: &AppState,
    note_path: &str,
    page_index: i64,
    ink_hash: &str,
    template_name: Option<String>,
    image: Vec<u8>,
) -> Result<IngestResponse> {
    let page_kind = classify_template(template_name.as_deref());
    let series: Option<MeetingSeries> = match page_kind {
        PageKind::Series(sid) => sqlx::query_as("SELECT * FROM meeting_series WHERE id = ?")
            .bind(sid)
            .fetch_optional(&state.pool)
            .await?,
        _ => None,
    };

    // Persist the page image for the review UI.
    let file_name = format!(
        "{}_{page_index:03}_{}.png",
        Utc::now().format("%Y%m%dT%H%M%S"),
        &ink_hash[..12.min(ink_hash.len())]
    );
    let image_path = state.config.pages_dir.join(&file_name);
    tokio::fs::write(&image_path, &image)
        .await
        .with_context(|| format!("writing {}", image_path.display()))?;

    // Zone spec matches what the templater printed for this series.
    let carried_ids = series.as_ref().map(|s| s.carried()).unwrap_or_default();
    let spec = TemplateSpec::new(&carried_ids);

    let people: Vec<Person> = sqlx::query_as("SELECT * FROM people ORDER BY name")
        .fetch_all(&state.pool)
        .await?;
    let areas: Vec<Area> = sqlx::query_as("SELECT * FROM areas ORDER BY name")
        .fetch_all(&state.pool)
        .await?;

    let transcribed = state
        .claude
        .transcribe_page(
            &state.config.transcribe_model,
            &image,
            &spec,
            &people,
            &areas,
        )
        .await?;

    let meeting_id =
        upsert_meeting(state, note_path, &page_kind, &series, &transcribed, &people).await?;

    // Resolve item names -> person ids (Claude normalizes spelling to the
    // directory; this maps to ids and surfaces misses in the review UI).
    let resolved: Vec<serde_json::Value> = transcribed
        .items
        .iter()
        .map(|item| {
            let resolve = |name: &Option<String>| {
                name.as_deref()
                    .and_then(|n| grammar::resolve_person(n, &people))
            };
            json!({
                "text": item.text,
                "kind": item.kind,
                "delegated_to": resolve(&item.delegated_to),
                "owed_to": resolve(&item.owed_to),
                "raise_with": resolve(&item.raise_with),
                "delegated_to_name": item.delegated_to,
                "owed_to_name": item.owed_to,
                "raise_with_name": item.raise_with,
                "priority": item.priority,
                "due_date": item.due_date,
            })
        })
        .collect();

    let raw = json!({
        "transcribed": transcribed,
        "resolved_items": resolved,
        "note_path": note_path,
        "page_index": page_index,
    });

    let transcription_id: i64 = sqlx::query_scalar(
        "INSERT INTO transcriptions (meeting_id, page_image_path, raw_json) \
         VALUES (?, ?, ?) RETURNING id",
    )
    .bind(meeting_id)
    .bind(file_name)
    .bind(raw.to_string())
    .fetch_one(&state.pool)
    .await?;

    // Record the dedup ledger entry.
    sqlx::query(
        "INSERT INTO page_state (note_path, page_index, ink_hash) VALUES (?, ?, ?) \
         ON CONFLICT (note_path, page_index) \
         DO UPDATE SET ink_hash = excluded.ink_hash, \
                       ingested_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
    )
    .bind(note_path)
    .bind(page_index)
    .bind(ink_hash)
    .execute(&state.pool)
    .await?;

    tracing::info!(transcription_id, ?meeting_id, note_path, page_index, "page ingested");
    Ok(IngestResponse {
        transcription_id,
        meeting_id,
    })
}

/// Get-or-create the meeting/reading session this page belongs to, merging
/// handwritten attendees (and title, for ad-hoc pages) into the row.
async fn upsert_meeting(
    state: &AppState,
    note_path: &str,
    page_kind: &PageKind,
    series: &Option<MeetingSeries>,
    transcribed: &Transcribed,
    people: &[Person],
) -> Result<Option<i64>> {
    let date = meeting_date(note_path);
    let written_title = transcribed
        .meeting_title
        .clone()
        .filter(|t| !t.trim().is_empty());

    let (kind, key, title, mut attendees) = match (page_kind, series) {
        (_, Some(s)) => (
            "meeting",
            format!("s{}_{date}", s.id),
            s.title.clone(),
            {
                let mut ids = s.attendees();
                if let Some(pid) = s.person_id {
                    ids.push(pid);
                }
                ids
            },
        ),
        (PageKind::Reading, _) => {
            // The "By:" line holds the author/creator, not attendees — fold
            // it into the title rather than resolving against people.
            let mut title = written_title.unwrap_or_else(|| "(untitled)".into());
            if !transcribed.attendees.is_empty() {
                title = format!("{title} — by {}", transcribed.attendees.join(", "));
            }
            (
                "reading",
                format!("reading_{date}_{}", slugify(&title)),
                title,
                Vec::new(),
            )
        }
        _ => {
            let title = written_title.unwrap_or_else(|| "(untitled)".into());
            (
                "meeting",
                format!("adhoc_{date}_{}", slugify(&title)),
                title,
                Vec::new(),
            )
        }
    };
    if kind == "meeting" {
        attendees.extend(
            transcribed
                .attendees
                .iter()
                .filter_map(|n| grammar::resolve_person(n, people)),
        );
    }
    attendees.sort_unstable();
    attendees.dedup();

    let existing: Option<(i64, String)> =
        sqlx::query_as("SELECT id, attendee_ids FROM meetings WHERE meeting_key = ?")
            .bind(&key)
            .fetch_optional(&state.pool)
            .await?;

    let id = match existing {
        Some((id, prior)) => {
            // Merge attendees discovered on later pages of the same meeting.
            let mut merged: Vec<i64> = serde_json::from_str(&prior).unwrap_or_default();
            merged.extend(attendees);
            merged.sort_unstable();
            merged.dedup();
            sqlx::query(
                "UPDATE meetings SET attendee_ids = ?, status = 'transcribed' WHERE id = ?",
            )
            .bind(serde_json::to_string(&merged).unwrap())
            .bind(id)
            .execute(&state.pool)
            .await?;
            id
        }
        None => {
            sqlx::query_scalar(
                "INSERT INTO meetings \
                 (meeting_key, kind, series_id, title, area_id, start_time, attendee_ids, status) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, 'transcribed') RETURNING id",
            )
            .bind(&key)
            .bind(kind)
            .bind(series.as_ref().map(|s| s.id))
            .bind(&title)
            .bind(series.as_ref().and_then(|s| s.area_id))
            .bind(format!("{date}T00:00:00Z"))
            .bind(serde_json::to_string(&attendees).unwrap())
            .fetch_one(&state.pool)
            .await?
        }
    };
    Ok(Some(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_names_classify() {
        assert_eq!(classify_template(Some("user_s3_1-1-priya")), PageKind::Series(3));
        assert_eq!(classify_template(Some("s12_infra-weekly.png")), PageKind::Series(12));
        assert_eq!(classify_template(Some("user_reading")), PageKind::Reading);
        assert_eq!(classify_template(Some("reading.png")), PageKind::Reading);
        assert_eq!(classify_template(Some("user_adhoc")), PageKind::AdHoc);
        assert_eq!(classify_template(Some("style_four_quadrant_method")), PageKind::AdHoc);
        assert_eq!(classify_template(Some("s_missing_id")), PageKind::AdHoc);
        assert_eq!(classify_template(None), PageKind::AdHoc);
    }

    #[test]
    fn meeting_date_from_daily_notebook_name() {
        assert_eq!(
            meeting_date("Note/20260720.note"),
            NaiveDate::from_ymd_opt(2026, 7, 20).unwrap()
        );
        assert_eq!(
            meeting_date("Note/20260618_074222.note"),
            NaiveDate::from_ymd_opt(2026, 6, 18).unwrap()
        );
        // Non-dated names fall back to today — just ensure no panic.
        let _ = meeting_date("Note/scratchpad.note");
    }
}
