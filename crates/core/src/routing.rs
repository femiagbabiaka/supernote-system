//! Routing rule: which open actions get pre-printed on a meeting's template.
//!
//! An open action appears on a meeting's sheet when either:
//! 1. **Series carry-over** — it originated in an earlier meeting of the same
//!    series (the classic "still-open items from previous sessions" strip), or
//! 2. **Person routing** — its `raise_with`, `owed_to`, or `delegated_to`
//!    person is in the room: an attendee of the meeting, or the counterpart of
//!    a 1:1 series. This is what carries "review X with NAME" onto the next
//!    meeting with NAME regardless of where it was written.

use crate::models::{Action, Meeting, MeetingSeries};

/// People considered "in the room" for a meeting.
pub fn people_in_room(meeting: &Meeting, series: Option<&MeetingSeries>) -> Vec<i64> {
    let mut ids = meeting.attendees();
    if let Some(s) = series {
        if s.is_one_on_one {
            if let Some(pid) = s.person_id {
                if !ids.contains(&pid) {
                    ids.push(pid);
                }
            }
        }
    }
    ids
}

/// Does `action` (with the series its origin meeting belongs to, if any)
/// belong on this meeting's pre-printed sheet?
pub fn routes_to(
    action: &Action,
    action_origin_series: Option<i64>,
    meeting: &Meeting,
    series: Option<&MeetingSeries>,
) -> bool {
    if action.status != "open" {
        return false;
    }
    // 1. Series carry-over.
    if let (Some(origin), Some(target)) = (action_origin_series, meeting.series_id) {
        if origin == target {
            return true;
        }
    }
    // 2. Person routing.
    let room = people_in_room(meeting, series);
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

    fn meeting(series_id: Option<i64>, attendees: &[i64]) -> Meeting {
        Meeting {
            id: 20,
            gcal_event_id: "evt".into(),
            series_id,
            title: "Weekly sync".into(),
            area_id: None,
            start_time: "2026-07-20T15:00:00Z".parse().unwrap(),
            end_time: "2026-07-20T16:00:00Z".parse().unwrap(),
            attendee_ids: serde_json::to_string(attendees).unwrap(),
            template_path: None,
            carried_ids: "[]".into(),
            status: "scheduled".into(),
        }
    }

    fn one_on_one(person: i64) -> MeetingSeries {
        MeetingSeries {
            id: 5,
            gcal_recurring_event_id: None,
            title: "1:1".into(),
            area_id: None,
            is_one_on_one: true,
            person_id: Some(person),
        }
    }

    #[test]
    fn closed_actions_never_route() {
        let a = action("done", Some(1), None);
        assert!(!routes_to(&a, Some(5), &meeting(Some(5), &[1]), None));
    }

    #[test]
    fn series_carry_over() {
        let a = action("open", None, None);
        assert!(routes_to(&a, Some(5), &meeting(Some(5), &[]), None));
        assert!(!routes_to(&a, Some(5), &meeting(Some(6), &[]), None));
        assert!(!routes_to(&a, None, &meeting(Some(5), &[]), None));
    }

    #[test]
    fn raise_with_routes_to_any_meeting_with_that_person() {
        // Action from series 5, but @person 7 attends an unrelated meeting.
        let a = action("open", Some(7), None);
        assert!(routes_to(&a, Some(5), &meeting(Some(9), &[7, 8]), None));
        assert!(!routes_to(&a, Some(5), &meeting(Some(9), &[8]), None));
    }

    #[test]
    fn one_on_one_counterpart_counts_as_in_room() {
        // Calendar attendees may be empty on a 1:1, but the series knows who.
        let a = action("open", Some(7), None);
        let s = one_on_one(7);
        assert!(routes_to(&a, None, &meeting(Some(5), &[]), Some(&s)));
    }

    #[test]
    fn delegated_to_routes_too() {
        let a = action("open", None, Some(3));
        assert!(routes_to(&a, None, &meeting(None, &[3]), None));
    }
}
