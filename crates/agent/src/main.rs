//! Ingest agent (runs on a systemd timer): scan the Drive-synced Note tree
//! for settled `.note` files, render ink layers via the Python renderer,
//! skip blank/unchanged pages, composite ink over the meeting template, and
//! POST the result to the webapp for transcription.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use clap::Parser;
use image::imageops::FilterType;
use image::{GrayImage, RgbaImage};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

#[derive(Debug, Parser)]
struct Config {
    /// Drive-synced directory containing .note files (e.g. <gdrive>/Note).
    #[arg(long, env = "SUPERNOTE_NOTE_DIR")]
    note_dir: PathBuf,
    /// Drive-synced MyStyle directory (template PNGs live here).
    #[arg(long, env = "SUPERNOTE_MYSTYLE_DIR")]
    mystyle_dir: PathBuf,
    /// Base URL of the supernote webapp.
    #[arg(long, env = "SUPERNOTE_WEBAPP_URL", default_value = "http://127.0.0.1:8130")]
    webapp_url: String,
    /// Renderer command; the note path and an output dir are appended.
    #[arg(long, env = "SUPERNOTE_RENDERER", default_value = "supernote-render")]
    renderer: String,
    /// Only ingest notes untouched for this many minutes (mid-edit guard).
    #[arg(long, env = "SUPERNOTE_DEBOUNCE_MINUTES", default_value = "120")]
    debounce_minutes: u64,
    /// Minimum ink coverage (fraction of pixels) for a page to count.
    #[arg(long, env = "SUPERNOTE_MIN_INK", default_value = "0.0005")]
    min_ink: f64,
}

/// Manifest emitted by render_note.py.
#[derive(Debug, Deserialize)]
struct Manifest {
    pages: Vec<ManifestPage>,
}

#[derive(Debug, Deserialize)]
struct ManifestPage {
    index: i64,
    path: String,
    template: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CheckResponse {
    new: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "supernote_agent=info".into()),
        )
        .init();
    let config = Config::parse();
    let http = reqwest::Client::new();

    let cutoff = SystemTime::now() - Duration::from_secs(config.debounce_minutes * 60);
    let mut candidates = Vec::new();
    for entry in WalkDir::new(&config.note_dir).follow_links(true) {
        let entry = entry?;
        if !entry.file_type().is_file()
            || entry.path().extension().and_then(|e| e.to_str()) != Some("note")
        {
            continue;
        }
        let mtime = entry.metadata()?.modified()?;
        if mtime <= cutoff {
            candidates.push(entry.path().to_path_buf());
        } else {
            tracing::debug!(path = %entry.path().display(), "still settling, skipped");
        }
    }
    tracing::info!(count = candidates.len(), "settled .note files found");

    let mut ingested = 0usize;
    for note in candidates {
        match process_note(&config, &http, &note).await {
            Ok(n) => ingested += n,
            Err(err) => tracing::error!(note = %note.display(), error = ?err, "note failed"),
        }
    }
    tracing::info!(ingested, "run complete");
    Ok(())
}

async fn process_note(config: &Config, http: &reqwest::Client, note: &Path) -> Result<usize> {
    // Note path relative to the tree — the stable dedup key.
    let rel = note
        .strip_prefix(&config.note_dir)
        .unwrap_or(note)
        .display()
        .to_string();

    let tmp = tempdir()?;
    let manifest = render_note(config, note, &tmp).await?;
    let mut ingested = 0usize;

    for page in &manifest.pages {
        let ink = image::open(&page.path)
            .with_context(|| format!("opening {}", page.path))?
            .to_rgba8();

        // Ink detection + hash over the alpha channel.
        let alpha: Vec<u8> = ink.pixels().map(|p| p.0[3]).collect();
        let coverage =
            alpha.iter().filter(|a| **a > 32).count() as f64 / alpha.len().max(1) as f64;
        if coverage < config.min_ink {
            tracing::debug!(page = page.index, coverage, "blank page skipped");
            continue;
        }
        let ink_hash = format!("{:x}", Sha256::digest(&alpha));

        // Dedup against the webapp's ledger.
        let check: CheckResponse = http
            .post(format!("{}/api/pages/check", config.webapp_url))
            .json(&serde_json::json!({
                "note_path": rel,
                "page_index": page.index,
                "ink_hash": ink_hash,
            }))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if !check.new {
            tracing::debug!(page = page.index, "already ingested");
            continue;
        }

        // Composite ink over the template background — the printed header is
        // what the transcriber reads, so this is load-bearing.
        let composited = composite(config, page, &ink)?;
        let mut png = Vec::new();
        composited.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;

        let mut form = reqwest::multipart::Form::new()
            .text("note_path", rel.clone())
            .text("page_index", page.index.to_string())
            .text("ink_hash", ink_hash)
            .part(
                "image",
                reqwest::multipart::Part::bytes(png)
                    .file_name("page.png")
                    .mime_str("image/png")?,
            );
        if let Some(t) = &page.template {
            form = form.text("template", t.clone());
        }
        let resp = http
            .post(format!("{}/api/ingest", config.webapp_url))
            .multipart(form)
            .send()
            .await?
            .error_for_status()?;
        let body: serde_json::Value = resp.json().await?;
        tracing::info!(
            note = rel,
            page = page.index,
            coverage = format!("{:.2}%", coverage * 100.0),
            transcription = body["transcription_id"].as_i64().unwrap_or(-1),
            meeting = ?body["meeting_id"].as_i64(),
            "page ingested"
        );
        ingested += 1;
    }
    Ok(ingested)
}

async fn render_note(config: &Config, note: &Path, out_dir: &Path) -> Result<Manifest> {
    let mut parts = config.renderer.split_whitespace();
    let program = parts.next().context("empty renderer command")?;
    let output = tokio::process::Command::new(program)
        .args(parts)
        .arg(note)
        .arg(out_dir)
        .output()
        .await
        .with_context(|| format!("running renderer {}", config.renderer))?;
    if !output.status.success() {
        bail!(
            "renderer failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    serde_json::from_slice(&output.stdout).context("parsing renderer manifest")
}

/// Draw the ink (alpha-encoded) over the template background, resizing ink to
/// the template's dimensions when the device page size differs.
fn composite(config: &Config, page: &ManifestPage, ink: &RgbaImage) -> Result<GrayImage> {
    let background: GrayImage = match &page.template {
        Some(name) => {
            let stem = name.strip_prefix("user_").unwrap_or(name);
            let path = config.mystyle_dir.join(format!("{stem}.png"));
            match image::open(&path) {
                Ok(img) => img.to_luma8(),
                Err(_) => {
                    tracing::warn!(template = name, "template PNG not found; using white");
                    GrayImage::from_pixel(ink.width(), ink.height(), image::Luma([255]))
                }
            }
        }
        None => GrayImage::from_pixel(ink.width(), ink.height(), image::Luma([255])),
    };

    let ink = if ink.dimensions() != background.dimensions() {
        image::imageops::resize(
            ink,
            background.width(),
            background.height(),
            FilterType::CatmullRom,
        )
    } else {
        ink.clone()
    };

    let mut out = background;
    for (x, y, p) in ink.enumerate_pixels() {
        let a = p.0[3] as u32;
        if a > 0 {
            let bg = out.get_pixel(x, y).0[0] as u32;
            // Ink is black; alpha blend over the background.
            let v = (bg * (255 - a)) / 255;
            out.put_pixel(x, y, image::Luma([v as u8]));
        }
    }
    Ok(out)
}

/// Minimal self-cleaning temp dir (avoids a tempfile dependency).
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl std::ops::Deref for TempDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

fn tempdir() -> Result<TempDir> {
    let dir = std::env::temp_dir().join(format!(
        "supernote-agent-{}-{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir)?;
    Ok(TempDir(dir))
}
