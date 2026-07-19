use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Area {
    pub id: i64,
    pub name: String,
    /// Comma-separated shorthand forms the transcriber may encounter.
    pub aliases: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Person {
    pub id: i64,
    pub name: String,
    pub aliases: String,
    pub email: Option<String>,
    pub area_id: Option<i64>,
}

impl Person {
    pub fn alias_list(&self) -> Vec<&str> {
        self.aliases
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct MeetingSeries {
    pub id: i64,
    pub gcal_recurring_event_id: Option<String>,
    pub title: String,
    pub area_id: Option<i64>,
    pub is_one_on_one: bool,
    pub person_id: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingStatus {
    Scheduled,
    Captured,
    Transcribed,
    Reviewed,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Meeting {
    pub id: i64,
    pub gcal_event_id: String,
    pub series_id: Option<i64>,
    pub title: String,
    pub area_id: Option<i64>,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    /// JSON array of people ids.
    pub attendee_ids: String,
    pub template_path: Option<String>,
    /// JSON array of action ids pre-printed on this meeting's template.
    pub carried_ids: String,
    pub status: String,
}

impl Meeting {
    pub fn attendees(&self) -> Vec<i64> {
        serde_json::from_str(&self.attendee_ids).unwrap_or_default()
    }

    pub fn carried(&self) -> Vec<i64> {
        serde_json::from_str(&self.carried_ids).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Action,
    Decision,
    Takeaway,
    Note,
    Research,
}

impl ActionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ActionKind::Action => "action",
            ActionKind::Decision => "decision",
            ActionKind::Takeaway => "takeaway",
            ActionKind::Note => "note",
            ActionKind::Research => "research",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Action {
    pub id: i64,
    pub text: String,
    pub meeting_id: Option<i64>,
    pub kind: String,
    pub delegated_to: Option<i64>,
    pub owed_to: Option<i64>,
    pub raise_with: Option<i64>,
    pub priority: i64,
    pub due_date: Option<String>,
    pub status: String,
    pub created_at: String,
    pub closed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ResearchReport {
    pub id: i64,
    pub action_id: i64,
    pub status: String,
    pub question: String,
    pub report_md: Option<String>,
    pub sources_json: String,
    pub pdf_path: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Transcription {
    pub id: i64,
    pub meeting_id: Option<i64>,
    pub page_image_path: Option<String>,
    pub raw_json: String,
    pub reviewed_json: Option<String>,
    pub status: String,
    pub created_at: String,
}
