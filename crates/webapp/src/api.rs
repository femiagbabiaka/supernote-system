//! JSON API consumed by the templater (series → template data) plus
//! people/areas/series seeding endpoints. There is no calendar integration:
//! standing meetings are seeded here, everything else comes off the page.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use supernote_core::models::{Action, Area, MeetingSeries, Person};
use supernote_core::routing;

use crate::state::{internal_error, AppState};

// ------------------------------------------------------------------ series

#[derive(Debug, Deserialize)]
pub struct NewSeries {
    pub title: String,
    pub area_id: Option<i64>,
    #[serde(default)]
    pub is_one_on_one: bool,
    /// Counterpart for 1:1 series.
    pub person_id: Option<i64>,
    /// Regular attendees (people ids) — drives action routing.
    #[serde(default)]
    pub attendee_ids: Vec<i64>,
}

pub async fn list_series(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<MeetingSeries>>, (StatusCode, String)> {
    sqlx::query_as("SELECT * FROM meeting_series ORDER BY title")
        .fetch_all(&state.pool)
        .await
        .map(Json)
        .map_err(|e| internal_error(e.into()))
}

/// Create-or-update by title, so seeding is re-runnable. Template bookkeeping
/// (template_path/carried_ids) is preserved on update.
pub async fn create_series(
    State(state): State<Arc<AppState>>,
    Json(s): Json<NewSeries>,
) -> Result<Json<MeetingSeries>, (StatusCode, String)> {
    sqlx::query_as(
        "INSERT INTO meeting_series (title, area_id, is_one_on_one, person_id, attendee_ids) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT (title) DO UPDATE SET \
           area_id = excluded.area_id, is_one_on_one = excluded.is_one_on_one, \
           person_id = excluded.person_id, attendee_ids = excluded.attendee_ids \
         RETURNING *",
    )
    .bind(&s.title)
    .bind(s.area_id)
    .bind(s.is_one_on_one)
    .bind(s.person_id)
    .bind(serde_json::to_string(&s.attendee_ids).unwrap())
    .fetch_one(&state.pool)
    .await
    .map(Json)
    .map_err(|e| internal_error(e.into()))
}

#[derive(Debug, Deserialize)]
pub struct SetTemplate {
    pub path: String,
    /// Action ids pre-printed in the carried-over table, in printed order.
    pub carried_ids: Vec<i64>,
}

pub async fn set_series_template(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(req): Json<SetTemplate>,
) -> Result<StatusCode, (StatusCode, String)> {
    sqlx::query("UPDATE meeting_series SET template_path = ?, carried_ids = ? WHERE id = ?")
        .bind(&req.path)
        .bind(serde_json::to_string(&req.carried_ids).unwrap())
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|e| internal_error(e.into()))?;
    Ok(StatusCode::NO_CONTENT)
}

// --------------------------------------------------------------- templates

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
pub struct SeriesTemplateData {
    pub series_id: i64,
    pub title: String,
    pub area: Option<String>,
    pub is_one_on_one: bool,
    /// 1:1 counterpart name, pre-printed on the "With:" line.
    pub person: Option<String>,
    pub carried: Vec<CarriedAction>,
}

/// For each series, the open actions to pre-print on its template.
/// Routing rule lives in `supernote_core::routing::routes_to_series`.
pub async fn templates(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<SeriesTemplateData>>, (StatusCode, String)> {
    let series: Vec<MeetingSeries> = sqlx::query_as("SELECT * FROM meeting_series ORDER BY title")
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

    let person_name = |id: Option<i64>| {
        id.and_then(|i| people.iter().find(|p| p.id == i))
            .map(|p| p.name.clone())
    };

    let out = series
        .iter()
        .map(|s| {
            let carried: Vec<CarriedAction> = open_actions
                .iter()
                .filter(|a| {
                    let origin = a.meeting_id.and_then(|mid| origin_series.get(&mid)).copied();
                    routing::routes_to_series(a, origin, s)
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
            SeriesTemplateData {
                series_id: s.id,
                title: s.title.clone(),
                area: s
                    .area_id
                    .and_then(|aid| areas.iter().find(|a| a.id == aid))
                    .map(|a| a.name.clone()),
                is_one_on_one: s.is_one_on_one,
                person: person_name(s.person_id),
                carried,
            }
        })
        .collect();
    Ok(Json(out))
}

// ----------------------------------------------------------------- seeding

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

/// Create-or-update by name, so seeding is re-runnable.
pub async fn create_person(
    State(state): State<Arc<AppState>>,
    Json(p): Json<NewPerson>,
) -> Result<Json<Person>, (StatusCode, String)> {
    sqlx::query_as(
        "INSERT INTO people (name, aliases, email, area_id) VALUES (?, ?, ?, ?) \
         ON CONFLICT (name) DO UPDATE SET \
           aliases = excluded.aliases, email = excluded.email, area_id = excluded.area_id \
         RETURNING *",
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

/// Create-or-update by name, so seeding is re-runnable.
pub async fn create_area(
    State(state): State<Arc<AppState>>,
    Json(a): Json<NewArea>,
) -> Result<Json<Area>, (StatusCode, String)> {
    sqlx::query_as(
        "INSERT INTO areas (name, aliases) VALUES (?, ?) \
         ON CONFLICT (name) DO UPDATE SET aliases = excluded.aliases \
         RETURNING *",
    )
    .bind(&a.name)
    .bind(&a.aliases)
    .fetch_one(&state.pool)
    .await
    .map(Json)
    .map_err(|e| internal_error(e.into()))
}
