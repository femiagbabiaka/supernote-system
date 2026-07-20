//! Apply a declarative seed file (areas, people, standing meeting series) to
//! the webapp. Entries reference each other by name; the tool resolves names
//! to ids and posts in dependency order. Re-running is safe: the webapp
//! upserts by name/title, so edits to the file propagate.
//!
//! Usage: supernote-seed seed.toml   (see seed.example.toml)

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Parser)]
struct Config {
    /// Path to the seed TOML file.
    file: PathBuf,
    /// Base URL of the supernote webapp.
    #[arg(long, env = "SUPERNOTE_WEBAPP_URL", default_value = "http://127.0.0.1:8130")]
    webapp_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Seed {
    #[serde(default)]
    areas: Vec<SeedArea>,
    #[serde(default)]
    people: Vec<SeedPerson>,
    #[serde(default)]
    series: Vec<SeedSeries>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeedArea {
    name: String,
    /// Comma-separated shorthand forms ("infra, plat").
    #[serde(default)]
    aliases: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeedPerson {
    name: String,
    /// Comma-separated aliases: initials, nicknames, common misreadings.
    #[serde(default)]
    aliases: String,
    email: Option<String>,
    /// Area name (must appear under [[areas]]).
    area: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeedSeries {
    title: String,
    /// Area name (must appear under [[areas]]).
    area: Option<String>,
    /// Person name — marks the series a 1:1 with them.
    one_on_one: Option<String>,
    /// Regular attendee names (people, not needed for the 1:1 counterpart).
    #[serde(default)]
    attendees: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::parse();
    let raw = std::fs::read_to_string(&config.file)
        .with_context(|| format!("reading {}", config.file.display()))?;
    let seed: Seed = toml::from_str(&raw).context("parsing seed file")?;
    let http = reqwest::Client::new();

    let post = |path: &'static str, body: serde_json::Value| {
        let http = http.clone();
        let url = format!("{}{path}", config.webapp_url);
        async move {
            let resp = http.post(&url).json(&body).send().await?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                bail!("POST {url} failed ({status}): {text}");
            }
            serde_json::from_str::<serde_json::Value>(&text).context("parsing response")
        }
    };

    // Areas first; remember name -> id.
    let mut area_ids: HashMap<String, i64> = HashMap::new();
    for a in &seed.areas {
        let v = post("/api/areas", json!({"name": a.name, "aliases": a.aliases})).await?;
        area_ids.insert(a.name.clone(), v["id"].as_i64().context("area id")?);
        println!("area    {} (#{})", a.name, v["id"]);
    }
    let area_id = |name: &Option<String>| -> Result<Option<i64>> {
        match name {
            None => Ok(None),
            Some(n) => area_ids
                .get(n)
                .copied()
                .map(Some)
                .with_context(|| format!("unknown area {n:?} — add it under [[areas]]")),
        }
    };

    // People next; remember name -> id.
    let mut person_ids: HashMap<String, i64> = HashMap::new();
    for p in &seed.people {
        let v = post(
            "/api/people",
            json!({
                "name": p.name,
                "aliases": p.aliases,
                "email": p.email,
                "area_id": area_id(&p.area)?,
            }),
        )
        .await?;
        person_ids.insert(p.name.clone(), v["id"].as_i64().context("person id")?);
        println!("person  {} (#{})", p.name, v["id"]);
    }
    let person_id = |name: &str| -> Result<i64> {
        person_ids
            .get(name)
            .copied()
            .with_context(|| format!("unknown person {name:?} — add them under [[people]]"))
    };

    // Series last.
    for s in &seed.series {
        let one_on_one = s.one_on_one.as_deref().map(person_id).transpose()?;
        let attendees: Vec<i64> = s
            .attendees
            .iter()
            .map(|n| person_id(n))
            .collect::<Result<_>>()?;
        let v = post(
            "/api/series",
            json!({
                "title": s.title,
                "area_id": area_id(&s.area)?,
                "is_one_on_one": one_on_one.is_some(),
                "person_id": one_on_one,
                "attendee_ids": attendees,
            }),
        )
        .await?;
        println!("series  {} (#{})", s.title, v["id"]);
    }

    println!(
        "\nseeded {} areas, {} people, {} series — run the templater (or wait \
         for the timer) to regenerate templates",
        seed.areas.len(),
        seed.people.len(),
        seed.series.len()
    );
    Ok(())
}
