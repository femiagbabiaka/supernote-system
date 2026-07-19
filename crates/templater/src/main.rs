//! Morning templater: read today's Google Calendar, upsert meetings into the
//! webapp, render one 1920×2560 template PNG per meeting (header + carried-over
//! action strip), and drop them into the Drive-synced MyStyle folder.

mod draw;
mod google;

use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{Local, NaiveDate};
use clap::Parser;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Parser)]
struct Config {
    /// Base URL of the supernote webapp.
    #[arg(long, env = "SUPERNOTE_WEBAPP_URL", default_value = "http://127.0.0.1:8130")]
    webapp_url: String,
    /// The Drive-synced MyStyle directory (templates land here).
    #[arg(long, env = "SUPERNOTE_MYSTYLE_DIR")]
    mystyle_dir: PathBuf,
    /// Google OAuth client secret JSON (desktop app credentials).
    #[arg(long, env = "GOOGLE_CLIENT_SECRET_FILE")]
    client_secret: PathBuf,
    /// Where the OAuth refresh token is cached after first consent.
    #[arg(long, env = "GOOGLE_TOKEN_CACHE")]
    token_cache: PathBuf,
    /// Calendar to read.
    #[arg(long, env = "GOOGLE_CALENDAR_ID", default_value = "primary")]
    calendar_id: String,
    /// Date to generate templates for (default: today, local time).
    #[arg(long)]
    date: Option<NaiveDate>,
    /// Directory containing `<font-name>-Regular.ttf` / `-Bold.ttf`.
    #[arg(long, env = "SUPERNOTE_FONT_DIR")]
    font_dir: PathBuf,
    #[arg(long, env = "SUPERNOTE_FONT_NAME", default_value = "LiberationSans")]
    font_name: String,
}

/// Mirror of the webapp's `MeetingTemplateData`.
#[derive(Debug, Deserialize)]
pub struct MeetingTemplateData {
    pub meeting_id: i64,
    pub title: String,
    pub series_title: Option<String>,
    pub area: Option<String>,
    pub start_time: String,
    pub end_time: String,
    pub carried: Vec<CarriedAction>,
}

#[derive(Debug, Deserialize)]
pub struct CarriedAction {
    pub action_id: i64,
    pub text: String,
    pub priority: i64,
    pub due_date: Option<String>,
    pub delegated_to: Option<String>,
    pub owed_to: Option<String>,
    pub raise_with: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "supernote_templater=info".into()),
        )
        .init();
    let config = Config::parse();
    let date = config.date.unwrap_or_else(|| Local::now().date_naive());
    let http = reqwest::Client::new();

    // 1. Calendar → expanded event instances for the day.
    let events = google::events_for_day(
        &config.client_secret,
        &config.token_cache,
        &config.calendar_id,
        date,
    )
    .await
    .context("fetching calendar events")?;
    tracing::info!(count = events.len(), %date, "calendar events fetched");

    // 2. Upsert into the webapp so it can route open actions.
    let upserted: Vec<serde_json::Value> = http
        .post(format!("{}/api/meetings/upsert", config.webapp_url))
        .json(&events)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("upserting meetings")?;
    tracing::info!(count = upserted.len(), "meetings upserted");

    // 3. Template data (carried-over actions per meeting, routing applied).
    let data: Vec<MeetingTemplateData> = http
        .get(format!("{}/api/templates", config.webapp_url))
        .query(&[("date", date.to_string())])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("fetching template data")?;

    // 4. Render one PNG per meeting, sorted into meeting order by filename.
    std::fs::create_dir_all(&config.mystyle_dir)?;
    let fonts = draw::Fonts::load(&config.font_dir, &config.font_name)?;
    for (i, meeting) in data.iter().enumerate() {
        let slug: String = meeting
            .title
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .split('-')
            .filter(|s| !s.is_empty())
            .take(5)
            .collect::<Vec<_>>()
            .join("-");
        let file_name = format!("{date}_{:02}_{slug}.png", i + 1);
        let path = config.mystyle_dir.join(&file_name);
        let image = draw::render_template(meeting, &fonts)?;
        image
            .save(&path)
            .with_context(|| format!("writing {}", path.display()))?;

        // Record the template path + printed carried ids on the meeting.
        let carried_ids: Vec<i64> = meeting.carried.iter().map(|c| c.action_id).collect();
        http.post(format!(
            "{}/api/meetings/{}/template",
            config.webapp_url, meeting.meeting_id
        ))
        .json(&json!({"path": file_name, "carried_ids": carried_ids}))
        .send()
        .await?
        .error_for_status()?;
        tracing::info!(meeting = %meeting.title, file = %path.display(), carried = carried_ids.len(), "template rendered");
    }

    Ok(())
}
