mod api;
mod claude;
mod dashboard;
mod ingest;
mod pdf;
mod research;
mod review;
mod state;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use axum::routing::{get, post};
use axum::Router;
use clap::Parser;

/// Supernote meeting/action web app: transcription, review, dashboard,
/// template data, research worker.
#[derive(Debug, Parser)]
pub struct Config {
    /// Path to the SQLite database.
    #[arg(long, env = "SUPERNOTE_DB", default_value = "supernote.sqlite3")]
    pub db: PathBuf,
    /// Listen address.
    #[arg(long, env = "SUPERNOTE_LISTEN", default_value = "127.0.0.1:8130")]
    pub listen: SocketAddr,
    /// Anthropic API key.
    #[arg(long, env = "ANTHROPIC_API_KEY", hide_env_values = true)]
    pub anthropic_api_key: String,
    /// Model used for handwriting transcription.
    #[arg(long, env = "SUPERNOTE_TRANSCRIBE_MODEL", default_value = "claude-opus-4-8")]
    pub transcribe_model: String,
    /// Model used for the deep-research pipeline.
    #[arg(long, env = "SUPERNOTE_RESEARCH_MODEL", default_value = "claude-opus-4-8")]
    pub research_model: String,
    /// Root of the rclone-mounted Google Drive Supernote tree. Research PDFs
    /// are written under `Document/Research/` inside it. Optional: without it
    /// reports are only available in the web UI.
    #[arg(long, env = "SUPERNOTE_GDRIVE_DIR")]
    pub gdrive_dir: Option<PathBuf>,
    /// Directory where ingested page images are stored.
    #[arg(long, env = "SUPERNOTE_PAGES_DIR", default_value = "pages")]
    pub pages_dir: PathBuf,
    /// Directory containing `<font-name>-Regular.ttf` etc., for PDF rendering.
    #[arg(long, env = "SUPERNOTE_FONT_DIR")]
    pub font_dir: Option<PathBuf>,
    /// Font family name for PDF rendering.
    #[arg(long, env = "SUPERNOTE_FONT_NAME", default_value = "LiberationSans")]
    pub font_name: String,
    /// Maximum research pipeline runs per day (cost guard).
    #[arg(long, env = "SUPERNOTE_RESEARCH_DAILY_CAP", default_value = "5")]
    pub research_daily_cap: i64,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "supernote_webapp=info,tower_http=info".into()),
        )
        .init();

    let config = Config::parse();
    std::fs::create_dir_all(&config.pages_dir)?;
    let pool = supernote_core::db::connect(&config.db).await?;
    let state = Arc::new(state::AppState::new(config, pool));

    tokio::spawn(research::worker(state.clone()));

    let app = Router::new()
        .route("/", get(|| async { axum::response::Redirect::to("/review") }))
        .route("/review", get(review::list))
        .route("/review/{id}", get(review::detail).post(review::save))
        .route("/dashboard", get(dashboard::show))
        .route("/pages/{name}", get(review::page_image))
        .route("/research/{id}/retry", post(research::retry))
        // JSON API consumed by the templater and ingest agent.
        .route("/api/pages/check", post(ingest::check_page))
        .route("/api/ingest", post(ingest::ingest))
        .route("/api/series", get(api::list_series).post(api::create_series))
        .route("/api/series/{id}/template", post(api::set_series_template))
        .route("/api/templates", get(api::templates))
        .route("/api/people", get(api::list_people).post(api::create_person))
        .route("/api/areas", get(api::list_areas).post(api::create_area))
        .with_state(state.clone());

    tracing::info!(listen = %state.config.listen, "supernote-webapp starting");
    let listener = tokio::net::TcpListener::bind(state.config.listen).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
