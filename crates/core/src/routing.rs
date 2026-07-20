//! Routing rules: which open actions get pre-printed where.
//!
//! Templates are generated per meeting *series* (standing meetings), so the
//! pre-print decision is made against the series — there is no calendar and
//! future meetings aren't known ahead of time. An open action lands on a
//! series' template when either:
//! 1. **Series carry-over** — it originated in a past meeting of that series
//!    (the "still-open items from previous sessions" strip), or
//! 2. **Person routing** — its `raise_with`, `owed_to`, or `delegated_to`
//!    person is expected in the room: a regular attendee of the series, or
//!    the counterpart of a 1:1. This is what carries "review X with NAME"
//!    onto the next meeting with NAME regardless of where it was written.

use crate::models::{Action, MeetingSeries};

/// People expected in the room for a series.
pub fn people_in_room(series: &MeetingSeries) -> Vec<i64> {
    let mut ids = series.attendees();
    if series.is_one_on_one {
        if let Some(pid) = series.person_id {
            if !ids.contains(&pid) {
                ids.push(pid);
            }
        }
    }
    ids
}

/// Does `action` (with the series its origin meeting belonged to, if any)
/// belong on this series' pre-printed template?
pub fn routes_to_series(
    action: &Action,
    action_origin_series: Option<i64>,
    series: &MeetingSeries,
) -> bool {
    if action.status != "open" {
        return false;
    }
    // 1. Series carry-over.
    if action_origin_series == Some(series.id) {
        return true;
    }
    // 2. Person routing.
    let room = people_in_room(series);
    [action.raise_with, action.owed_to, action.delegated_to]
        .into_iter()
        .flatten()
        .any(|pid| room.contains(&pid))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(status: &str, raise_with: Option<i64>, delegated_to: Option<i64>) -> Action {
        Action {
            id: 1,
            text: "test".into(),
            meeting_id: Some(10),
            kind: "action".into(),
            delegated_to,
            owed_to: None,
            raise_with,
            priority: 0,
            due_date: None,
            status: status.into(),
            created_at: "2026-07-01T00:00:00Z".into(),
            closed_at: None,
        }
    }

    fn series(id: i64, attendees: &[i64]) -> MeetingSeries {
        MeetingSeries {
            id,
            title: format!("series {id}"),
            area_id: None,
            is_one_on_one: false,
            person_id: None,
            attendee_ids: serde_json::to_string(attendees).unwrap(),
            template_path: None,
            carried_ids: "[]".into(),
        }
    }

    fn one_on_one(id: i64, person: i64) -> MeetingSeries {
        MeetingSeries {
            is_one_on_one: true,
            person_id: Some(person),
            ..series(id, &[])
        }
    }

    #[test]
    fn closed_actions_never_route() {
        let a = action("done", Some(1), None);
        assert!(!routes_to_series(&a, Some(5), &series(5, &[1])));
    }

    #[test]
    fn series_carry_over() {
        let a = action("open", None, None);
        assert!(routes_to_series(&a, Some(5), &series(5, &[])));
        assert!(!routes_to_series(&a, Some(5), &series(6, &[])));
        assert!(!routes_to_series(&a, None, &series(5, &[])));
    }

    #[test]
    fn raise_with_routes_to_any_series_with_that_person() {
        // Action from series 5, but @person 7 is a regular of series 9.
        let a = action("open", Some(7), None);
        assert!(routes_to_series(&a, Some(5), &series(9, &[7, 8])));
        assert!(!routes_to_series(&a, Some(5), &series(9, &[8])));
    }

    #[test]
    fn one_on_one_counterpart_counts_as_in_room() {
        let a = action("open", Some(7), None);
        assert!(routes_to_series(&a, None, &one_on_one(5, 7)));
        assert!(!routes_to_series(&a, None, &one_on_one(5, 8)));
    }

    #[test]
    fn delegated_to_routes_too() {
        let a = action("open", None, Some(3));
        assert!(routes_to_series(&a, None, &series(2, &[3])));
    }
}
