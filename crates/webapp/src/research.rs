//! Deep-research worker: picks up confirmed `research` actions, runs the
//! Claude web-search pipeline, renders a PDF into the Drive tree so it syncs
//! back onto the device.

use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Redirect;
use chrono::Utc;

use crate::state::{internal_error, AppState};

const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Background loop: single-flight, oldest-pending-first, per-day cost cap.
pub async fn worker(state: Arc<AppState>) {
    loop {
        if let Err(err) = tick(&state).await {
            tracing::error!(error = ?err, "research worker tick failed");
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn tick(state: &AppState) -> Result<()> {
    // Cost guard: cap the number of runs *started* today.
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let started_today: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM research_reports \
         WHERE status IN ('running', 'ready', 'failed') AND date(created_at) = ?",
    )
    .bind(&today)
    .fetch_one(&state.pool)
    .await?;
    if started_today >= state.config.research_daily_cap {
        return Ok(());
    }

    let pending: Option<(i64, i64, String)> = sqlx::query_as(
        "SELECT id, action_id, question FROM research_reports \
         WHERE status = 'pending' ORDER BY created_at LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await?;
    let Some((id, action_id, question)) = pending else {
        return Ok(());
    };

    sqlx::query("UPDATE research_reports SET status = 'running' WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await?;
    tracing::info!(report = id, %question, "research run starting");

    match run(state, action_id, &question).await {
        Ok((report_md, sources_json, pdf_path)) => {
            sqlx::query(
                "UPDATE research_reports SET status = 'ready', report_md = ?, \
                 sources_json = ?, pdf_path = ?, \
                 completed_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?",
            )
            .bind(report_md)
            .bind(sources_json)
            .bind(pdf_path)
            .bind(id)
            .execute(&state.pool)
            .await?;
            tracing::info!(report = id, "research run complete");
        }
        Err(err) => {
            sqlx::query("UPDATE research_reports SET status = 'failed', error = ? WHERE id = ?")
                .bind(format!("{err:#}"))
                .bind(id)
                .execute(&state.pool)
                .await?;
            tracing::error!(report = id, error = ?err, "research run failed");
        }
    }
    Ok(())
}

async fn run(
    state: &AppState,
    action_id: i64,
    question: &str,
) -> Result<(String, String, Option<String>)> {
    // Meeting context helps the refiner scope the question.
    let context: Option<(String,)> = sqlx::query_as(
        "SELECT m.title || ' on ' || m.start_time FROM actions a \
         JOIN meetings m ON m.id = a.meeting_id WHERE a.id = ?",
    )
    .bind(action_id)
    .fetch_optional(&state.pool)
    .await?;
    let context = context.map(|(c,)| c).unwrap_or_else(|| "none".into());

    let (report_md, sources) = state
        .claude
        .research(&state.config.research_model, question, &context)
        .await?;
    let sources_json = serde_json::to_string(&sources)?;

    // Render the PDF into the Drive tree so it syncs onto the Manta.
    let pdf_path = match &state.config.gdrive_dir {
        Some(root) => {
            let dir = root.join("Document").join("Research");
            tokio::fs::create_dir_all(&dir)
                .await
                .with_context(|| format!("creating {}", dir.display()))?;
            let slug: String = question
                .to_lowercase()
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { '-' })
                .collect::<String>()
                .split('-')
                .filter(|s| !s.is_empty())
                .take(6)
                .collect::<Vec<_>>()
                .join("-");
            let path = dir.join(format!("{}_{slug}.pdf", Utc::now().format("%Y-%m-%d")));
            let title = question.to_string();
            let md = report_md.clone();
            let srcs = sources.clone();
            let font_dir = state.config.font_dir.clone();
            let font_name = state.config.font_name.clone();
            let out = path.clone();
            // genpdf is synchronous; keep it off the async runtime.
            tokio::task::spawn_blocking(move || {
                crate::pdf::render_report(&out, &title, &md, &srcs, font_dir.as_deref(), &font_name)
            })
            .await??;
            Some(path.display().to_string())
        }
        None => None,
    };

    Ok((report_md, sources_json, pdf_path))
}

/// `POST /research/{id}/retry` — requeue a failed run from the dashboard.
pub async fn retry(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Redirect, (StatusCode, String)> {
    sqlx::query("UPDATE research_reports SET status = 'pending', error = NULL WHERE id = ? AND status = 'failed'")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|e| internal_error(e.into()))?;
    Ok(Redirect::to("/dashboard"))
}
