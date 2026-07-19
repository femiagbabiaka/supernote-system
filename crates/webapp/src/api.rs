//! JSON API consumed by the templater (calendar → meetings → template data)
//! plus minimal people/areas seeding endpoints.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use supernote_core::models::{Action, Area, Meeting, MeetingSeries, Person};
use supernote_core::routing;

use crate::state::{internal_error, AppState};

// ---------------------------------------------------------------- meetings

/// One calendar event instance, as seen by the templater.
#[derive(Debug, Deserialize)]
pub struct CalendarEvent {
    pub gcal_event_id: String,
    /// recurringEventId when the instance belongs to a series.
    pub gcal_recurring_event_id: Option<String>,
    pub title: String,
    pub start_time: String, // RFC 3339
    pub end_time: String,
    /// Attendee email addresses from the calendar.
    #[serde(default)]
    pub attendee_emails: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct UpsertedMeeting {
    pub id: i64,
    pub gcal_event_id: String,
}

pub async fn upsert_meetings(
    State(state): State<Arc<AppState>>,
    Json(events): Json<Vec<CalendarEvent>>,
) -> Result<Json<Vec<UpsertedMeeting>>, (StatusCode, String)> {
    let people: Vec<Person> = sqlx::query_as("SELECT * FROM people")
        .fetch_all(&state.pool)
        .await
        .map_err(|e| internal_error(e.into()))?;

    let mut out = Vec::with_capacity(events.len());
    for ev in events {
        // Resolve series (create on first sight of a recurring event).
        let series_id: Option<i64> = match &ev.gcal_recurring_event_id {
            Some(rid) => {
                let existing: Option<(i64,)> = sqlx::query_as(
                    "SELECT id FROM meeting_series WHERE gcal_recurring_event_id = ?",
                )
                .bind(rid)
                .fetch_optional(&state.pool)
                .await
                .map_err(|e| internal_error(e.into()))?;
                match existing {
                    Some((id,)) => Some(id),
                    None => {
                        // Heuristic: a recurring meeting with exactly one
                        // non-self attendee is a 1:1 with that person.
                        let others: Vec<i64> = ev
                            .attendee_emails
                            .iter()
                            .filter_map(|e| {
                                people
                                    .iter()
                                    .find(|p| p.email.as_deref() == Some(e.as_str()))
                                    .map(|p| p.id)
                            })
                            .collect();
                        let (is_one_on_one, person_id) = if others.len() == 1 {
                            (true, Some(others[0]))
                        } else {
                            (false, None)
                        };
                        let id: i64 = sqlx::query_scalar(
                            "INSERT INTO meeting_series \
                             (gcal_recurring_event_id, title, is_one_on_one, person_id) \
                             VALUES (?, ?, ?, ?) RETURNING id",
                        )
                        .bind(rid)
                        .bind(&ev.title)
                        .bind(is_one_on_one)
                        .bind(person_id)
                        .fetch_one(&state.pool)
                        .await
                        .map_err(|e| internal_error(e.into()))?;
                        Some(id)
                    }
                }
            }
            None => None,
        };

        let attendee_ids: Vec<i64> = ev
            .attendee_emails
            .iter()
            .filter_map(|e| {
                people
                    .iter()
                    .find(|p| p.email.as_deref() == Some(e.as_str()))
                    .map(|p| p.id)
            })
            .collect();

        let id: i64 = sqlx::query_scalar(
            "INSERT INTO meetings (gcal_event_id, series_id, title, start_time, end_time, attendee_ids) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT (gcal_event_id) DO UPDATE SET \
               series_id = excluded.series_id, title = excluded.title, \
               start_time = excluded.start_time, end_time = excluded.end_time, \
               attendee_ids = excluded.attendee_ids \
             RETURNING id",
        )
        .bind(&ev.gcal_event_id)
        .bind(series_id)
        .bind(&ev.title)
        .bind(&ev.start_time)
        .bind(&ev.end_time)
        .bind(serde_json::to_string(&attendee_ids).unwrap())
        .fetch_one(&state.pool)
        .await
        .map_err(|e| internal_error(e.into()))?;

        out.push(UpsertedMeeting {
            id,
            gcal_event_id: ev.gcal_event_id,
        });
    }
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
pub struct SetTemplate {
    pub path: String,
    /// Action ids pre-printed in the carried-over table, in printed order.
    pub carried_ids: Vec<i64>,
}

pub async fn set_template_path(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(req): Json<SetTemplate>,
) -> Result<StatusCode, (StatusCode, String)> {
    sqlx::query("UPDATE meetings SET template_path = ?, carried_ids = ? WHERE id = ?")
        .bind(&req.path)
        .bind(serde_json::to_string(&req.carried_ids).unwrap())
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|e| internal_error(e.into()))?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- templates

#[derive(Debug, Deserialize)]
pub struct TemplatesQuery {
    /// ISO date (YYYY-MM-DD); templates are generated for this day's meetings.
    pub date: String,
}

#[derive(Debug, Serialize)]
pub struct CarriedAction {
    pub action_id: i64,
    pub text: String,
    pub priority: i64,
    pub due_date: Option<String>,
    pub delegated_to: Option<String>,
    pub owed_to: Option<String>,
    pub raise_with: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MeetingTemplateData {
    pub meeting_id: i64,
    pub gcal_event_id: String,
    pub title: String,
    pub series_title: Option<String>,
    pub area: Option<String>,
    pub start_time: String,
    pub end_time: String,
    pub carried: Vec<CarriedAction>,
}

/// For each meeting on the given date, the open actions to pre-print.
/// Routing rule lives in `supernote_core::routing`.
pub async fn templates_for_date(
    State(state): State<Arc<AppState>>,
    Query(q): Query<TemplatesQuery>,
) -> Result<Json<Vec<MeetingTemplateData>>, (StatusCode, String)> {
    let meetings: Vec<Meeting> = sqlx::query_as(
        "SELECT * FROM meetings WHERE date(start_time) = ? ORDER BY start_time",
    )
    .bind(&q.date)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| internal_error(e.into()))?;

    let series: Vec<MeetingSeries> = sqlx::query_as("SELECT * FROM meeting_series")
        .fetch_all(&state.pool)
        .await
        .map_err(|e| internal_error(e.into()))?;
    let areas: Vec<Area> = sqlx::query_as("SELECT * FROM areas")
        .fetch_all(&state.pool)
        .await
        .map_err(|e| internal_error(e.into()))?;
    let people: Vec<Person> = sqlx::query_as("SELECT * FROM people")
        .fetch_all(&state.pool)
        .await
        .map_err(|e| internal_error(e.into()))?;
    // Open actions joined with the series of their origin meeting.
    let open_actions: Vec<Action> = sqlx::query_as("SELECT * FROM actions WHERE status = 'open'")
        .fetch_all(&state.pool)
        .await
        .map_err(|e| internal_error(e.into()))?;
    let origin_series: std::collections::HashMap<i64, i64> = sqlx::query_as::<_, (i64, i64)>(
        "SELECT id, series_id FROM meetings WHERE series_id IS NOT NULL",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| internal_error(e.into()))?
    .into_iter()
    .collect();

    let person_name =
        |id: Option<i64>| id.and_then(|i| people.iter().find(|p| p.id == i)).map(|p| p.name.clone());

    let mut out = Vec::with_capacity(meetings.len());
    for meeting in &meetings {
        let s = meeting
            .series_id
            .and_then(|sid| series.iter().find(|s| s.id == sid));
        let carried: Vec<CarriedAction> = open_actions
            .iter()
            .filter(|a| {
                let origin = a.meeting_id.and_then(|mid| origin_series.get(&mid)).copied();
                routing::routes_to(a, origin, meeting, s)
            })
            .map(|a| CarriedAction {
                action_id: a.id,
                text: a.text.clone(),
                priority: a.priority,
                due_date: a.due_date.clone(),
                delegated_to: person_name(a.delegated_to),
                owed_to: person_name(a.owed_to),
                raise_with: person_name(a.raise_with),
            })
            .collect();

        out.push(MeetingTemplateData {
            meeting_id: meeting.id,
            gcal_event_id: meeting.gcal_event_id.clone(),
            title: meeting.title.clone(),
            series_title: s.map(|s| s.title.clone()),
            area: meeting
                .area_id
                .or(s.and_then(|s| s.area_id))
                .and_then(|aid| areas.iter().find(|a| a.id == aid))
                .map(|a| a.name.clone()),
            start_time: meeting.start_time.to_rfc3339(),
            end_time: meeting.end_time.to_rfc3339(),
            carried,
        });
    }
    Ok(Json(out))
}

// ---------------------------------------------------------------- seeding

#[derive(Debug, Deserialize)]
pub struct NewPerson {
    pub name: String,
    #[serde(default)]
    pub aliases: String,
    pub email: Option<String>,
    pub area_id: Option<i64>,
}

pub async fn list_people(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<Person>>, (StatusCode, String)> {
    sqlx::query_as("SELECT * FROM people ORDER BY name")
        .fetch_all(&state.pool)
        .await
        .map(Json)
        .map_err(|e| internal_error(e.into()))
}

pub async fn create_person(
    State(state): State<Arc<AppState>>,
    Json(p): Json<NewPerson>,
) -> Result<Json<Person>, (StatusCode, String)> {
    sqlx::query_as(
        "INSERT INTO people (name, aliases, email, area_id) VALUES (?, ?, ?, ?) RETURNING *",
    )
    .bind(&p.name)
    .bind(&p.aliases)
    .bind(&p.email)
    .bind(p.area_id)
    .fetch_one(&state.pool)
    .await
    .map(Json)
    .map_err(|e| internal_error(e.into()))
}

#[derive(Debug, Deserialize)]
pub struct NewArea {
    pub name: String,
    #[serde(default)]
    pub aliases: String,
}

pub async fn list_areas(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<Area>>, (StatusCode, String)> {
    sqlx::query_as("SELECT * FROM areas ORDER BY name")
        .fetch_all(&state.pool)
        .await
        .map(Json)
        .map_err(|e| internal_error(e.into()))
}

pub async fn create_area(
    State(state): State<Arc<AppState>>,
    Json(a): Json<NewArea>,
) -> Result<Json<Area>, (StatusCode, String)> {
    sqlx::query_as("INSERT INTO areas (name, aliases) VALUES (?, ?) RETURNING *")
        .bind(&a.name)
        .bind(&a.aliases)
        .fetch_one(&state.pool)
        .await
        .map(Json)
        .map_err(|e| internal_error(e.into()))
}
