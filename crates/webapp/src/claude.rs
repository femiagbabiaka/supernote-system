//! Raw-HTTP Anthropic API client (no official Rust SDK exists).
//!
//! Two call paths:
//! - `transcribe_page`: vision + structured outputs (`output_config.format`
//!   with a JSON schema) → guaranteed-parseable transcription JSON.
//! - `research`: web_search server tool + adaptive thinking, continuing on
//!   `pause_turn`, collecting cited sources from `web_search_tool_result`
//!   blocks.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

use supernote_core::grammar;
use supernote_core::models::{Area, Person};
use supernote_core::template_spec::TemplateSpec;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

pub struct Claude {
    http: reqwest::Client,
    api_key: String,
}

/// One transcribed handwritten line, names still unresolved (strings).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscribedItem {
    pub text: String,
    pub kind: String,
    pub delegated_to: Option<String>,
    pub owed_to: Option<String>,
    pub raise_with: Option<String>,
    pub priority: i64,
    pub due_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CarriedTick {
    pub action_id: i64,
    pub ticked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcribed {
    pub header_text: String,
    pub carried_ticks: Vec<CarriedTick>,
    pub items: Vec<TranscribedItem>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub url: String,
    pub title: String,
}

impl Claude {
    pub fn new(api_key: String) -> Self {
        Claude {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(600))
                .build()
                .expect("reqwest client"),
            api_key,
        }
    }

    /// POST /v1/messages with retry on 429/5xx/529.
    async fn messages(&self, body: &Value) -> Result<Value> {
        let mut last_err = None;
        for attempt in 0..4u32 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(2u64.pow(attempt))).await;
            }
            let resp = self
                .http
                .post(API_URL)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", API_VERSION)
                .json(body)
                .send()
                .await;
            match resp {
                Ok(r) => {
                    let status = r.status();
                    let text = r.text().await.unwrap_or_default();
                    if status.is_success() {
                        return serde_json::from_str(&text).context("parsing API response");
                    }
                    let retryable = status.as_u16() == 429 || status.as_u16() >= 500;
                    let err = anyhow!("API error {status}: {text}");
                    if !retryable {
                        return Err(err);
                    }
                    last_err = Some(err);
                }
                Err(e) => last_err = Some(anyhow!(e).context("sending API request")),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("API request failed")))
    }

    /// Transcribe one composited page image into structured JSON.
    pub async fn transcribe_page(
        &self,
        model: &str,
        png: &[u8],
        spec: &TemplateSpec,
        people: &[Person],
        areas: &[Area],
    ) -> Result<Transcribed> {
        let people_list: Vec<String> = people
            .iter()
            .map(|p| {
                let aliases = p.alias_list().join(", ");
                if aliases.is_empty() {
                    p.name.clone()
                } else {
                    format!("{} (aliases: {aliases})", p.name)
                }
            })
            .collect();
        let area_list: Vec<String> = areas
            .iter()
            .map(|a| {
                if a.aliases.is_empty() {
                    a.name.clone()
                } else {
                    format!("{} (aliases: {})", a.name, a.aliases)
                }
            })
            .collect();

        let system = format!(
            "You transcribe handwritten meeting notes captured on a Supernote e-ink tablet. \
             The page has pre-printed template zones (header, carried-over action rows with \
             tick-boxes and printed #id markers) and a freehand writing area. \
             Zone geometry (pixel coords): {}\n\n\
             {}\n\n\
             People directory (resolve handwritten names to these exact spellings):\n{}\n\n\
             Topic areas:\n{}\n\n\
             Return: header_text (the printed header you can read), carried_ticks (one entry \
             per printed carried-over row, action_id from its printed #id, ticked=true only \
             if the box is clearly marked), items (one per distinct handwritten line/thought, \
             classified and with markers stripped into fields; use null for absent fields; \
             due_date as YYYY-MM-DD), and a one-sentence summary of the page.",
            spec.to_prompt_json(),
            grammar::prompt_block(),
            people_list.join("\n"),
            area_list.join("\n"),
        );

        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "header_text": {"type": "string"},
                "carried_ticks": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "action_id": {"type": "integer"},
                            "ticked": {"type": "boolean"}
                        },
                        "required": ["action_id", "ticked"]
                    }
                },
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "text": {"type": "string"},
                            "kind": {"type": "string", "enum": ["action", "decision", "takeaway", "note", "research"]},
                            "delegated_to": {"anyOf": [{"type": "string"}, {"type": "null"}]},
                            "owed_to": {"anyOf": [{"type": "string"}, {"type": "null"}]},
                            "raise_with": {"anyOf": [{"type": "string"}, {"type": "null"}]},
                            "priority": {"type": "integer"},
                            "due_date": {"anyOf": [{"type": "string"}, {"type": "null"}]}
                        },
                        "required": ["text", "kind", "delegated_to", "owed_to", "raise_with", "priority", "due_date"]
                    }
                },
                "summary": {"type": "string"}
            },
            "required": ["header_text", "carried_ticks", "items", "summary"]
        });

        let body = json!({
            "model": model,
            "max_tokens": 16000,
            "thinking": {"type": "adaptive"},
            "system": system,
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": base64::engine::general_purpose::STANDARD.encode(png),
                        }
                    },
                    {"type": "text", "text": "Transcribe this meeting-notes page."}
                ]
            }],
            "output_config": {"format": {"type": "json_schema", "schema": schema}},
        });

        let resp = self.messages(&body).await?;
        let text = first_text(&resp)
            .ok_or_else(|| anyhow!("no text block in transcription response: {resp}"))?;
        serde_json::from_str(&text).context("parsing structured transcription")
    }

    /// Run the deep-research pipeline: refine the question, then search the
    /// web and synthesize a cited Markdown report.
    pub async fn research(&self, model: &str, question: &str, context: &str) -> Result<(String, Vec<Source>)> {
        // Step 1: refine the raw handwritten request into a research question.
        let refine = json!({
            "model": model,
            "max_tokens": 1024,
            "thinking": {"type": "adaptive"},
            "system": "Rewrite the user's raw meeting-note research request into a single, \
                       specific, self-contained research question. Reply with the question only.",
            "messages": [{"role": "user", "content": format!("Request: {question}\nMeeting context: {context}")}],
        });
        let refined = first_text(&self.messages(&refine).await?)
            .unwrap_or_else(|| question.to_string());
        tracing::info!(%refined, "research question refined");

        // Step 2: web search + synthesis, continuing on pause_turn.
        let system = "You are a research analyst preparing a briefing for an engineering \
                      area tech lead. Research the question thoroughly using web search. \
                      Verify claims across sources where possible. Produce a Markdown report: \
                      a short executive summary, key findings with inline source attribution, \
                      trade-offs/risks, and a recommendation. Be concrete and cite sources.";
        let mut messages = vec![json!({"role": "user", "content": refined})];
        let mut sources: Vec<Source> = Vec::new();
        let mut report = String::new();

        for _round in 0..6 {
            let body = json!({
                "model": model,
                "max_tokens": 16000,
                "thinking": {"type": "adaptive"},
                "system": system,
                "messages": messages,
                "tools": [{"type": "web_search_20260209", "name": "web_search", "max_uses": 8}],
            });
            let resp = self.messages(&body).await?;
            collect_sources(&resp, &mut sources);
            let stop = resp["stop_reason"].as_str().unwrap_or_default();
            if stop == "pause_turn" {
                // Server-side tool loop paused; echo the assistant turn and resume.
                messages.push(json!({"role": "assistant", "content": resp["content"]}));
                continue;
            }
            report = all_text(&resp);
            break;
        }
        if report.trim().is_empty() {
            bail!("research produced no report text");
        }
        sources.dedup_by(|a, b| a.url == b.url);
        Ok((report, sources))
    }
}

fn first_text(resp: &Value) -> Option<String> {
    resp["content"]
        .as_array()?
        .iter()
        .find(|b| b["type"] == "text")
        .and_then(|b| b["text"].as_str())
        .map(str::to_string)
}

fn all_text(resp: &Value) -> String {
    resp["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn collect_sources(resp: &Value, out: &mut Vec<Source>) {
    let Some(blocks) = resp["content"].as_array() else {
        return;
    };
    for block in blocks {
        if block["type"] == "web_search_tool_result" {
            if let Some(results) = block["content"].as_array() {
                for r in results {
                    if let (Some(url), Some(title)) = (r["url"].as_str(), r["title"].as_str()) {
                        if !out.iter().any(|s| s.url == url) {
                            out.push(Source {
                                url: url.to_string(),
                                title: title.to_string(),
                            });
                        }
                    }
                }
            }
        }
    }
}
