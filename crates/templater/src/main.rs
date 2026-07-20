//! Templater: for each standing meeting series, render a 1920×2560 template
//! PNG (pre-printed title + carried-over action strip + "With:" write-in
//! line) into the Drive-synced MyStyle folder, plus one generic ad-hoc
//! template with blank "Meeting:" / "With:" lines.
//!
//! There is no calendar integration by design: meeting identity comes from
//! the chosen template + the handwritten header, so the only data leaving
//! the machine is what gets written on the page.

mod draw;

use std::path::PathBuf;

use anyhow::{Context, Result};
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
    /// Directory containing `<font-name>-Regular.ttf` / `-Bold.ttf`.
    #[arg(long, env = "SUPERNOTE_FONT_DIR")]
    font_dir: PathBuf,
    #[arg(long, env = "SUPERNOTE_FONT_NAME", default_value = "LiberationSans")]
    font_name: String,
}

/// Mirror of the webapp's `SeriesTemplateData`.
#[derive(Debug, Deserialize)]
pub struct SeriesTemplateData {
    pub series_id: i64,
    pub title: String,
    pub area: Option<String>,
    pub is_one_on_one: bool,
    pub person: Option<String>,
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "supernote_templater=info".into()),
        )
        .init();
    let config = Config::parse();
    let http = reqwest::Client::new();

    let data: Vec<SeriesTemplateData> = http
        .get(format!("{}/api/templates", config.webapp_url))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("fetching template data (is the webapp running?)")?;

    std::fs::create_dir_all(&config.mystyle_dir)?;
    let fonts = draw::Fonts::load(&config.font_dir, &config.font_name)?;

    // One template per series, named s<id>_<slug> so ingest can map the
    // page's chosen background back to the series.
    for series in &data {
        let file_name = format!("s{}_{}.png", series.series_id, slugify(&series.title));
        let path = config.mystyle_dir.join(&file_name);
        let job = draw::TemplateJob {
            labels: ("Meeting:", "With:"),
            title: Some(series.title.clone()),
            with: series.person.clone().filter(|_| series.is_one_on_one),
            area: series.area.clone(),
            carried: &series.carried,
        };
        draw::render_template(&job, &fonts)?
            .save(&path)
            .with_context(|| format!("writing {}", path.display()))?;

        let carried_ids: Vec<i64> = series.carried.iter().map(|c| c.action_id).collect();
        http.post(format!(
            "{}/api/series/{}/template",
            config.webapp_url, series.series_id
        ))
        .json(&json!({"path": file_name, "carried_ids": carried_ids}))
        .send()
        .await?
        .error_for_status()?;
        tracing::info!(series = %series.title, file = %path.display(), carried = carried_ids.len(), "series template rendered");
    }

    // Generic ad-hoc template: blank Meeting/With lines, no carried strip.
    let adhoc = draw::TemplateJob {
        labels: ("Meeting:", "With:"),
        title: None,
        with: None,
        area: None,
        carried: &[],
    };
    let path = config.mystyle_dir.join("adhoc.png");
    draw::render_template(&adhoc, &fonts)?
        .save(&path)
        .with_context(|| format!("writing {}", path.display()))?;
    tracing::info!(file = %path.display(), "ad-hoc template rendered");

    // Reading/listening notes template: Title/By lines, no carried strip.
    let reading = draw::TemplateJob {
        labels: ("Title:", "By:"),
        title: None,
        with: None,
        area: None,
        carried: &[],
    };
    let path = config.mystyle_dir.join("reading.png");
    draw::render_template(&reading, &fonts)?
        .save(&path)
        .with_context(|| format!("writing {}", path.display()))?;
    tracing::info!(file = %path.display(), "reading template rendered");

    Ok(())
}
