//! Ingest API used by the ingest agent:
//! - `POST /api/pages/check` — dedup probe against the page_state ledger.
//! - `POST /api/ingest` — multipart page image + metadata → transcription.

use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use supernote_core::grammar;
use supernote_core::models::{Area, Person};
use supernote_core::template_spec::TemplateSpec;

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
    let mut meeting_id: Option<i64> = None;
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
            "meeting_id" => meeting_id = field.text().await.ok().and_then(|s| s.parse().ok()),
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

    process_page(
        &state, &note_path, page_index, &ink_hash, meeting_id, template_name, image,
    )
    .await
    .map(Json)
    .map_err(internal_error)
}

async fn process_page(
    state: &AppState,
    note_path: &str,
    page_index: i64,
    ink_hash: &str,
    explicit_meeting: Option<i64>,
    template_name: Option<String>,
    image: Vec<u8>,
) -> Result<IngestResponse> {
    // Resolve the meeting: explicit id > template file-name match.
    let meeting_id = match explicit_meeting {
        Some(id) => Some(id),
        None => match &template_name {
            Some(name) => {
                // Device reports user templates as "user_<file stem>".
                let stem = name.strip_prefix("user_").unwrap_or(name);
                sqlx::query_as::<_, (i64,)>(
                    "SELECT id FROM meetings WHERE template_path LIKE '%' || ? || '%' \
                     ORDER BY start_time DESC LIMIT 1",
                )
                .bind(stem)
                .fetch_optional(&state.pool)
                .await?
                .map(|(id,)| id)
            }
            None => None,
        },
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

    // Carried-over ids printed on this meeting's template, recorded by the
    // templater — lets us hand Claude the exact zone spec the page was drawn with.
    let carried_ids: Vec<i64> = match meeting_id {
        Some(mid) => sqlx::query_as::<_, (String,)>("SELECT carried_ids FROM meetings WHERE id = ?")
            .bind(mid)
            .fetch_optional(&state.pool)
            .await?
            .map(|(s,)| serde_json::from_str(&s).unwrap_or_default())
            .unwrap_or_default(),
        None => Vec::new(),
    };
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

    // Resolve names -> person ids server-side (Claude already normalizes
    // spelling to the directory, this maps to ids and catches misses).
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

    // Record the dedup ledger entry and bump meeting status.
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

    if let Some(mid) = meeting_id {
        sqlx::query("UPDATE meetings SET status = 'transcribed' WHERE id = ? AND status IN ('scheduled', 'captured')")
            .bind(mid)
            .execute(&state.pool)
            .await?;
    }

    tracing::info!(transcription_id, ?meeting_id, note_path, page_index, "page ingested");
    Ok(IngestResponse {
        transcription_id,
        meeting_id,
    })
}
