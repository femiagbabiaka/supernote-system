//! Open-actions dashboard.

use std::sync::Arc;

use askama::Template;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Html;

use crate::state::{internal_error, AppState};

pub struct ActionRow {
    pub id: i64,
    pub text: String,
    pub kind: String,
    pub meeting: String,
    pub area: String,
    pub delegated_to: String,
    pub owed_to: String,
    pub raise_with: String,
    pub priority: i64,
    pub due_date: String,
    pub research_status: String,
    pub research_id: i64, // 0 = none
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardPage {
    open: Vec<ActionRow>,
    done_recent: Vec<ActionRow>,
}

type Row = (
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    Option<String>,
    Option<String>,
    Option<i64>,
);

const BASE_QUERY: &str = "SELECT a.id, a.text, a.kind, m.title, ar.name, \
        pd.name, po.name, pr.name, a.priority, a.due_date, r.status, r.id \
     FROM actions a \
     LEFT JOIN meetings m ON m.id = a.meeting_id \
     LEFT JOIN meeting_series s ON s.id = m.series_id \
     LEFT JOIN areas ar ON ar.id = COALESCE(m.area_id, s.area_id) \
     LEFT JOIN people pd ON pd.id = a.delegated_to \
     LEFT JOIN people po ON po.id = a.owed_to \
     LEFT JOIN people pr ON pr.id = a.raise_with \
     LEFT JOIN research_reports r ON r.action_id = a.id";

fn to_row(r: Row) -> ActionRow {
    let (id, text, kind, meeting, area, pd, po, pr, priority, due, rstatus, rid) = r;
    ActionRow {
        id,
        text,
        kind,
        meeting: meeting.unwrap_or_default(),
        area: area.unwrap_or_default(),
        delegated_to: pd.unwrap_or_default(),
        owed_to: po.unwrap_or_default(),
        raise_with: pr.unwrap_or_default(),
        priority,
        due_date: due.unwrap_or_default(),
        research_status: rstatus.unwrap_or_default(),
        research_id: rid.unwrap_or(0),
    }
}

pub async fn show(
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, (StatusCode, String)> {
    let open: Vec<Row> = sqlx::query_as(&format!(
        "{BASE_QUERY} WHERE a.status = 'open' \
         ORDER BY a.priority DESC, a.due_date IS NULL, a.due_date, a.created_at"
    ))
    .fetch_all(&state.pool)
    .await
    .map_err(|e| internal_error(e.into()))?;

    let done: Vec<Row> = sqlx::query_as(&format!(
        "{BASE_QUERY} WHERE a.status = 'done' ORDER BY a.closed_at DESC LIMIT 15"
    ))
    .fetch_all(&state.pool)
    .await
    .map_err(|e| internal_error(e.into()))?;

    let page = DashboardPage {
        open: open.into_iter().map(to_row).collect(),
        done_recent: done.into_iter().map(to_row).collect(),
    };
    page.render()
        .map(Html)
        .map_err(|e| internal_error(e.into()))
}
