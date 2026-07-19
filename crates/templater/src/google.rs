//! Google Calendar access: installed-app OAuth (cached refresh token) and
//! `events.list` with `singleEvents=true` — the API expands recurrences and
//! resolves cancellations/moves, avoiding the classic ICS-feed traps.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{Local, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};

/// The shape the webapp's `/api/meetings/upsert` expects.
#[derive(Debug, Serialize)]
pub struct CalendarEvent {
    pub gcal_event_id: String,
    pub gcal_recurring_event_id: Option<String>,
    pub title: String,
    pub start_time: String,
    pub end_time: String,
    pub attendee_emails: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct EventsResponse {
    #[serde(default)]
    items: Vec<Item>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Item {
    id: String,
    #[serde(default)]
    status: String,
    summary: Option<String>,
    #[serde(rename = "recurringEventId")]
    recurring_event_id: Option<String>,
    start: Option<When>,
    end: Option<When>,
    #[serde(default)]
    attendees: Vec<Attendee>,
}

#[derive(Debug, Deserialize)]
struct When {
    #[serde(rename = "dateTime")]
    date_time: Option<String>,
    #[allow(dead_code)]
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Attendee {
    email: Option<String>,
    #[serde(rename = "self", default)]
    is_self: bool,
    #[serde(rename = "responseStatus", default)]
    response_status: String,
}

async fn access_token(client_secret: &Path, token_cache: &Path) -> Result<String> {
    let secret = yup_oauth2::read_application_secret(client_secret)
        .await
        .with_context(|| format!("reading client secret {}", client_secret.display()))?;
    let auth = yup_oauth2::InstalledFlowAuthenticator::builder(
        secret,
        yup_oauth2::InstalledFlowReturnMethod::HTTPRedirect,
    )
    .persist_tokens_to_disk(token_cache)
    .build()
    .await
    .context("building Google authenticator")?;
    let token = auth
        .token(&["https://www.googleapis.com/auth/calendar.readonly"])
        .await
        .context("obtaining Google access token (run interactively once to consent)")?;
    token
        .token()
        .map(str::to_string)
        .context("authenticator returned no access token")
}

/// Fetch the day's meetings: timed, non-cancelled, recurrences expanded.
pub async fn events_for_day(
    client_secret: &Path,
    token_cache: &Path,
    calendar_id: &str,
    date: NaiveDate,
) -> Result<Vec<CalendarEvent>> {
    let token = access_token(client_secret, token_cache).await?;
    let day_start = Local
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .context("ambiguous local midnight")?;
    let day_end = day_start + chrono::Duration::days(1);

    let http = reqwest::Client::new();
    let url = format!(
        "https://www.googleapis.com/calendar/v3/calendars/{}/events",
        urlencode(calendar_id)
    );

    let mut events = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let mut query = vec![
            ("timeMin".to_string(), day_start.to_rfc3339()),
            ("timeMax".to_string(), day_end.to_rfc3339()),
            // Expand recurring events into instances; cancelled/moved
            // instances resolve correctly (no phantom meetings).
            ("singleEvents".to_string(), "true".to_string()),
            ("orderBy".to_string(), "startTime".to_string()),
            ("maxResults".to_string(), "100".to_string()),
        ];
        if let Some(t) = &page_token {
            query.push(("pageToken".to_string(), t.clone()));
        }
        let resp: EventsResponse = http
            .get(&url)
            .bearer_auth(&token)
            .query(&query)
            .send()
            .await?
            .error_for_status()
            .context("calendar events.list")?
            .json()
            .await?;

        for item in resp.items {
            if item.status == "cancelled" {
                continue;
            }
            // All-day entries (date without dateTime) aren't meetings.
            let (Some(start), Some(end)) = (
                item.start.as_ref().and_then(|w| w.date_time.clone()),
                item.end.as_ref().and_then(|w| w.date_time.clone()),
            ) else {
                continue;
            };
            let attendee_emails = item
                .attendees
                .iter()
                .filter(|a| !a.is_self && a.response_status != "declined")
                .filter_map(|a| a.email.clone())
                .collect();
            events.push(CalendarEvent {
                gcal_event_id: item.id,
                gcal_recurring_event_id: item.recurring_event_id,
                title: item.summary.unwrap_or_else(|| "(untitled)".into()),
                start_time: start,
                end_time: end,
                attendee_emails,
            });
        }

        page_token = resp.next_page_token;
        if page_token.is_none() {
            break;
        }
    }
    Ok(events)
}

fn urlencode(s: &str) -> String {
    s.replace('@', "%40")
}
