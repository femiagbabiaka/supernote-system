use sqlx::SqlitePool;

use crate::claude::Claude;
use crate::Config;

pub struct AppState {
    pub config: Config,
    pub pool: SqlitePool,
    pub claude: Claude,
}

impl AppState {
    pub fn new(config: Config, pool: SqlitePool) -> Self {
        let claude = Claude::new(config.anthropic_api_key.clone());
        AppState {
            config,
            pool,
            claude,
        }
    }
}

/// Convert an anyhow error into a 500 response, logging it.
pub fn internal_error(err: anyhow::Error) -> (axum::http::StatusCode, String) {
    tracing::error!(error = ?err, "request failed");
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        format!("internal error: {err:#}"),
    )
}
