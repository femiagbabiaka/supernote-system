//! Review UI: skim transcribed pages, fix the odd word, save.
//! Saving finalizes actions, closes ticked carried-over items, and enqueues
//! research runs for confirmed `research` items.

use std::sync::Arc;

use askama::Template;
use axum::extract::{Path, RawForm, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde_json::Value;

use supernote_core::models::Person;

use crate::state::{internal_error, AppState};

fn render<T: Template>(t: T) -> Result<Html<String>, (StatusCode, String)> {
    t.render().map(Html).map_err(|e| internal_error(e.into()))
}

// ------------------------------------------------------------------- list

struct ReviewRow {
    id: i64,
    meeting: String,
    created_at: String,
    summary: String,
}

#[derive(Template)]
#[template(path = "review_list.html")]
struct ReviewListPage {
    rows: Vec<ReviewRow>,
}

pub async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, (StatusCode, String)> {
    let raw: Vec<(i64, Option<String>, String, String)> = sqlx::query_as(
        "SELECT t.id, m.title, t.created_at, t.raw_json \
         FROM transcriptions t LEFT JOIN meetings m ON m.id = t.meeting_id \
         WHERE t.status = 'awaiting_review' ORDER BY t.created_at",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| internal_error(e.into()))?;

    let rows = raw
        .into_iter()
        .map(|(id, meeting, created_at, raw_json)| {
            let parsed: Value = serde_json::from_str(&raw_json).unwrap_or_default();
            ReviewRow {
                id,
                meeting: meeting.unwrap_or_else(|| "(unassigned)".into()),
                created_at,
                summary: parsed["transcribed"]["summary"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
            }
        })
        .collect();
    render(ReviewListPage { rows })
}

// ----------------------------------------------------------------- detail

struct ItemRow {
    index: usize,
    text: String,
    kind: String,
    delegated_to: i64, // 0 = none
    owed_to: i64,
    raise_with: i64,
    priority: i64,
    due_date: String,
    unresolved: String, // names Claude saw but we couldn't resolve
}

struct TickRow {
    action_id: i64,
    text: String,
    ticked: bool,
}

struct PersonOpt {
    id: i64,
    name: String,
}

struct MeetingOpt {
    id: i64,
    label: String,
    selected: bool,
}

#[derive(Template)]
#[template(path = "review_detail.html")]
struct ReviewDetailPage {
    id: i64,
    meeting: String,
    header_text: String,
    summary: String,
    image: String,
    items: Vec<ItemRow>,
    ticks: Vec<TickRow>,
    people: Vec<PersonOpt>,
    kinds: Vec<String>,
    meetings: Vec<MeetingOpt>,
    unassigned: bool,
}

pub async fn detail(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Html<String>, (StatusCode, String)> {
    let row: Option<(i64, Option<i64>, Option<String>, String)> = sqlx::query_as(
        "SELECT t.id, t.meeting_id, t.page_image_path, t.raw_json \
         FROM transcriptions t WHERE t.id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| internal_error(e.into()))?;
    let Some((id, meeting_id, image, raw_json)) = row else {
        return Err((StatusCode::NOT_FOUND, "no such transcription".into()));
    };

    let raw: Value = serde_json::from_str(&raw_json).unwrap_or_default();
    let people: Vec<Person> = sqlx::query_as("SELECT * FROM people ORDER BY name")
        .fetch_all(&state.pool)
        .await
        .map_err(|e| internal_error(e.into()))?;

    let meeting: Option<(String,)> = match meeting_id {
        Some(mid) => sqlx::query_as("SELECT title FROM meetings WHERE id = ?")
            .bind(mid)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| internal_error(e.into()))?,
        None => None,
    };

    // Recent meetings for manual assignment when the page didn't match.
    let recent: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT id, title, start_time FROM meetings ORDER BY start_time DESC LIMIT 20",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| internal_error(e.into()))?;

    let items = raw["resolved_items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(index, it)| {
            let mut unresolved = Vec::new();
            for (idk, namek) in [
                ("delegated_to", "delegated_to_name"),
                ("owed_to", "owed_to_name"),
                ("raise_with", "raise_with_name"),
            ] {
                if it[idk].is_null() {
                    if let Some(n) = it[namek].as_str() {
                        unresolved.push(n.to_string());
                    }
                }
            }
            ItemRow {
                index,
                text: it["text"].as_str().unwrap_or("").to_string(),
                kind: it["kind"].as_str().unwrap_or("action").to_string(),
                delegated_to: it["delegated_to"].as_i64().unwrap_or(0),
                owed_to: it["owed_to"].as_i64().unwrap_or(0),
                raise_with: it["raise_with"].as_i64().unwrap_or(0),
                priority: it["priority"].as_i64().unwrap_or(0),
                due_date: it["due_date"].as_str().unwrap_or("").to_string(),
                unresolved: unresolved.join(", "),
            }
        })
        .collect();

    // Carried ticks: join printed rows with the action text.
    let mut ticks = Vec::new();
    if let Some(list) = raw["transcribed"]["carried_ticks"].as_array() {
        for t in list {
            let action_id = t["action_id"].as_i64().unwrap_or(0);
            let text: Option<(String,)> = sqlx::query_as("SELECT text FROM actions WHERE id = ?")
                .bind(action_id)
                .fetch_optional(&state.pool)
                .await
                .map_err(|e| internal_error(e.into()))?;
            ticks.push(TickRow {
                action_id,
                text: text.map(|(t,)| t).unwrap_or_else(|| "(unknown)".into()),
                ticked: t["ticked"].as_bool().unwrap_or(false),
            });
        }
    }

    render(ReviewDetailPage {
        id,
        meeting: meeting
            .map(|(t,)| t)
            .unwrap_or_else(|| "(unassigned)".into()),
        header_text: {
            let title = raw["transcribed"]["meeting_title"].as_str().unwrap_or("");
            let attendees: Vec<&str> = raw["transcribed"]["attendees"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            if attendees.is_empty() {
                title.to_string()
            } else {
                format!("{title} — with {}", attendees.join(", "))
            }
        },
        summary: raw["transcribed"]["summary"].as_str().unwrap_or("").to_string(),
        image: image.unwrap_or_default(),
        items,
        ticks,
        people: people
            .into_iter()
            .map(|p| PersonOpt {
                id: p.id,
                name: p.name,
            })
            .collect(),
        kinds: ["action", "decision", "takeaway", "note", "research"].map(String::from).to_vec(),
        meetings: recent
            .into_iter()
            .map(|(mid, title, start)| MeetingOpt {
                id: mid,
                label: format!("{title} ({start})"),
                selected: meeting_id == Some(mid),
            })
            .collect(),
        unassigned: meeting_id.is_none(),
    })
}

// ------------------------------------------------------------------- save

pub async fn save(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    RawForm(body): RawForm,
) -> Result<Redirect, (StatusCode, String)> {
    let pairs: Vec<(String, String)> =
        serde_urlencoded::from_bytes(&body).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let get = |key: &str| -> Option<&str> {
        pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };

    let meeting_id: Option<i64> = get("meeting_id").and_then(|v| v.parse().ok()).filter(|v| *v > 0);

    // Collect item indices present in the form.
    let mut indices: Vec<usize> = pairs
        .iter()
        .filter_map(|(k, _)| {
            k.strip_prefix("item.")
                .and_then(|rest| rest.split('.').next())
                .and_then(|i| i.parse().ok())
        })
        .collect();
    indices.sort_unstable();
    indices.dedup();

    let mut created = 0usize;
    for i in indices {
        let f = |field: &str| get(&format!("item.{i}.{field}"));
        if f("delete").is_some() {
            continue;
        }
        let text = f("text").unwrap_or("").trim().to_string();
        if text.is_empty() {
            continue;
        }
        // Dedup against existing open actions with identical text.
        let dup: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM actions WHERE status = 'open' AND text = ?")
                .bind(&text)
                .fetch_optional(&state.pool)
                .await
                .map_err(|e| internal_error(e.into()))?;
        if dup.is_some() {
            continue;
        }

        let kind = f("kind").unwrap_or("action").to_string();
        let person = |field: &str| -> Option<i64> {
            f(field).and_then(|v| v.parse::<i64>().ok()).filter(|v| *v > 0)
        };
        let priority: i64 = f("priority").and_then(|v| v.parse().ok()).unwrap_or(0);
        let due = f("due_date").map(str::trim).filter(|s| !s.is_empty());

        let action_id: i64 = sqlx::query_scalar(
            "INSERT INTO actions (text, meeting_id, kind, delegated_to, owed_to, raise_with, priority, due_date) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(&text)
        .bind(meeting_id)
        .bind(&kind)
        .bind(person("delegated_to"))
        .bind(person("owed_to"))
        .bind(person("raise_with"))
        .bind(priority)
        .bind(due)
        .fetch_one(&state.pool)
        .await
        .map_err(|e| internal_error(e.into()))?;
        created += 1;

        // A confirmed research item enqueues a deep-research run.
        if kind == "research" {
            sqlx::query(
                "INSERT INTO research_reports (action_id, question) VALUES (?, ?) \
                 ON CONFLICT (action_id) DO NOTHING",
            )
            .bind(action_id)
            .bind(&text)
            .execute(&state.pool)
            .await
            .map_err(|e| internal_error(e.into()))?;
        }
    }

    // Ticked carried-over rows close their origin action.
    let mut closed = 0usize;
    for (k, v) in &pairs {
        if let Some(aid) = k.strip_prefix("tick.") {
            if v == "on" || v == "true" {
                if let Ok(aid) = aid.parse::<i64>() {
                    sqlx::query(
                        "UPDATE actions SET status = 'done', \
                         closed_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') \
                         WHERE id = ? AND status = 'open'",
                    )
                    .bind(aid)
                    .execute(&state.pool)
                    .await
                    .map_err(|e| internal_error(e.into()))?;
                    closed += 1;
                }
            }
        }
    }

    let reviewed = serde_json::json!({ "form": pairs });
    sqlx::query(
        "UPDATE transcriptions SET status = 'reviewed', reviewed_json = ?, meeting_id = COALESCE(?, meeting_id) WHERE id = ?",
    )
    .bind(reviewed.to_string())
    .bind(meeting_id)
    .bind(id)
    .execute(&state.pool)
    .await
    .map_err(|e| internal_error(e.into()))?;

    if let Some(mid) = meeting_id {
        sqlx::query("UPDATE meetings SET status = 'reviewed' WHERE id = ?")
            .bind(mid)
            .execute(&state.pool)
            .await
            .map_err(|e| internal_error(e.into()))?;
    }

    tracing::info!(transcription = id, created, closed, "review saved");
    Ok(Redirect::to("/review"))
}

// -------------------------------------------------------------- page image

pub async fn page_image(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Response, (StatusCode, String)> {
    // Only serve plain file names out of the pages dir.
    let safe = std::path::Path::new(&name)
        .file_name()
        .ok_or((StatusCode::BAD_REQUEST, "bad name".into()))?;
    let path = state.config.pages_dir.join(safe);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, "no such page".into()))?;
    Ok(([(header::CONTENT_TYPE, "image/png")], bytes).into_response())
}
