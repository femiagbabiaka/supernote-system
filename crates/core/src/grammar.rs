//! The handwritten symbol grammar — single source of truth.
//!
//! Used in two places that must stay in sync:
//! 1. The Claude vision prompt (`prompt_block`) that teaches the transcriber
//!    what each mark means.
//! 2. The text parser (`parse_line`) used to (re)derive structured fields when
//!    an action line is edited in the review UI, and in unit tests.
//!
//! Marks (unicode form / ASCII fallback):
//!   → name   |  -> name    delegate to person
//!   ◎ name   |  (o) name   deliverable owed to person
//!   @ name                 raise with person at next meeting with them
//!   !                      priority (repeat to escalate: !! > !)
//!   ? ...                  research request (leading): delegate deep research
//!   due: DATE              due date (ISO 2026-07-30 or M/D)
//!   D: / T: / N:           explicit kind prefix: decision / takeaway / note
//!   ☐ / ☑                  carried-over item tick-boxes (printed on templates)

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::models::{ActionKind, Person};

/// A structured reading of one handwritten action line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParsedAction {
    /// The action text with marker annotations stripped.
    pub text: String,
    pub kind: ActionKind,
    pub delegated_to: Option<i64>,
    pub owed_to: Option<i64>,
    pub raise_with: Option<i64>,
    pub priority: u8,
    pub due_date: Option<NaiveDate>,
}

/// Grammar description block embedded in the Claude vision prompt.
pub fn prompt_block() -> &'static str {
    "Handwritten lines may carry these marks (also accept the ASCII fallbacks):\n\
     - `→ name` or `-> name`: the task is DELEGATED to that person (delegated_to).\n\
     - `◎ name` or `(o) name`: a deliverable is OWED to that person (owed_to).\n\
     - `@ name`: raise/discuss this with that person at the next meeting with them (raise_with).\n\
       Natural phrasings like \"raise X with NAME\" or \"review X with NAME\" mean the same.\n\
     - `!`: priority; more exclamation marks mean higher priority.\n\
     - a leading `?`, or phrasings like \"research/look into/dig into X\": a RESEARCH request\n\
       (kind = research) to be delegated to a research agent.\n\
     - `due: DATE`, `due DATE`, or a bare date: the due date.\n\
     - a leading `D:` marks a decision, `T:` a takeaway, `N:` a note; otherwise classify\n\
       from content into action/decision/takeaway/note.\n\
     - In the printed carried-over table, `☑` (ticked box) means that item is DONE;\n\
       `☐` (empty box) means still open.\n\
     Resolve names against the people list provided; prefer spelling from the list over\n\
     the literal handwriting. Strip marker syntax from the returned action text."
}

/// Resolve a name fragment against the people directory (case-insensitive,
/// matches full name, first name, or any alias). Longest match wins.
pub fn resolve_person(fragment: &str, people: &[Person]) -> Option<i64> {
    let frag = fragment.trim().to_lowercase();
    if frag.is_empty() {
        return None;
    }
    let mut best: Option<(usize, i64)> = None;
    for p in people {
        let mut candidates: Vec<String> = vec![p.name.to_lowercase()];
        if let Some(first) = p.name.split_whitespace().next() {
            candidates.push(first.to_lowercase());
        }
        for a in p.alias_list() {
            candidates.push(a.to_lowercase());
        }
        for c in candidates {
            if frag == c || frag.starts_with(&format!("{c} ")) {
                let len = c.len();
                if best.map(|(l, _)| len > l).unwrap_or(true) {
                    best = Some((len, p.id));
                }
            }
        }
    }
    best.map(|(_, id)| id)
}

/// Markers that terminate a name capture.
const MARKERS: &[&str] = &["→", "->", "◎", "(o)", "@", "!", "due:", "due "];

fn find_next_marker(s: &str) -> usize {
    MARKERS
        .iter()
        .filter_map(|m| s.find(m))
        .min()
        .unwrap_or(s.len())
}

fn parse_due(s: &str, today: NaiveDate) -> Option<NaiveDate> {
    let s = s.trim().trim_end_matches(['.', ',', ';']);
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(d);
    }
    // M/D — resolve to this year, or next year if already past.
    let parts: Vec<&str> = s.splitn(2, '/').collect();
    if parts.len() == 2 {
        if let (Ok(m), Ok(d)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
            if let Some(date) = NaiveDate::from_ymd_opt(today.year(), m, d) {
                return Some(if date < today {
                    NaiveDate::from_ymd_opt(today.year() + 1, m, d)?
                } else {
                    date
                });
            }
        }
    }
    None
}

/// Parse a single handwritten/transcribed line into a structured action.
pub fn parse_line(line: &str, people: &[Person], today: NaiveDate) -> ParsedAction {
    let mut rest = line.trim().to_string();
    let mut kind = ActionKind::Action;

    // Kind prefixes.
    for (prefix, k) in [
        ("?", ActionKind::Research),
        ("D:", ActionKind::Decision),
        ("T:", ActionKind::Takeaway),
        ("N:", ActionKind::Note),
    ] {
        if let Some(stripped) = rest.strip_prefix(prefix) {
            kind = k;
            rest = stripped.trim().to_string();
            break;
        }
    }

    let mut delegated_to = None;
    let mut owed_to = None;
    let mut raise_with = None;
    let mut priority: u8 = 0;
    let mut due_date = None;
    let mut text = String::new();

    let mut s = rest.as_str();
    while !s.is_empty() {
        let mut matched = false;
        for (marker, slot) in [
            ("→", 0u8),
            ("->", 0),
            ("◎", 1),
            ("(o)", 1),
            ("@", 2),
        ] {
            if let Some(after) = s.strip_prefix(marker) {
                let after = after.trim_start();
                let end = find_next_marker(after);
                let (name, tail) = after.split_at(end);
                let id = resolve_person(name, people);
                match slot {
                    0 => delegated_to = delegated_to.or(id),
                    1 => owed_to = owed_to.or(id),
                    _ => raise_with = raise_with.or(id),
                }
                // Unresolved names stay in the text so the reviewer notices.
                if id.is_none() && !name.trim().is_empty() {
                    text.push_str(name.trim());
                    text.push(' ');
                }
                s = tail;
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }
        if let Some(after) = s.strip_prefix('!') {
            priority = priority.saturating_add(1);
            s = after;
            continue;
        }
        for due_marker in ["due:", "due "] {
            if s
                .get(..due_marker.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(due_marker))
            {
                let after = s[due_marker.len()..].trim_start();
                let end = after
                    .find(|c: char| c.is_whitespace())
                    .unwrap_or(after.len());
                let (candidate, tail) = after.split_at(end);
                if let Some(d) = parse_due(candidate, today) {
                    due_date = Some(d);
                    s = tail;
                } else {
                    text.push_str(&s[..due_marker.len()]);
                    s = &s[due_marker.len()..];
                }
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }
        // Plain character: copy through.
        let ch = s.chars().next().unwrap();
        text.push(ch);
        s = &s[ch.len_utf8()..];
    }

    ParsedAction {
        text: text.split_whitespace().collect::<Vec<_>>().join(" "),
        kind,
        delegated_to,
        owed_to,
        raise_with,
        priority,
        due_date,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn people() -> Vec<Person> {
        vec![
            Person {
                id: 1,
                name: "Alice Chen".into(),
                aliases: "AC".into(),
                email: None,
                area_id: None,
            },
            Person {
                id: 2,
                name: "Bob Kowalski".into(),
                aliases: "bobby,BK".into(),
                email: None,
                area_id: None,
            },
            Person {
                id: 3,
                name: "Priya Natarajan".into(),
                aliases: "".into(),
                email: None,
                area_id: None,
            },
        ]
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 7, 19).unwrap()
    }

    #[test]
    fn plain_action() {
        let p = parse_line("Follow up on capacity plan", &people(), today());
        assert_eq!(p.text, "Follow up on capacity plan");
        assert_eq!(p.kind, ActionKind::Action);
        assert_eq!(p.priority, 0);
        assert!(p.delegated_to.is_none());
    }

    #[test]
    fn delegation_unicode_and_ascii() {
        for line in ["Draft RFC → Alice", "Draft RFC -> Alice"] {
            let p = parse_line(line, &people(), today());
            assert_eq!(p.delegated_to, Some(1), "line: {line}");
            assert_eq!(p.text, "Draft RFC");
        }
    }

    #[test]
    fn owed_and_alias_resolution() {
        let p = parse_line("Send headcount summary ◎ bobby", &people(), today());
        assert_eq!(p.owed_to, Some(2));
        let p = parse_line("Send summary (o) BK", &people(), today());
        assert_eq!(p.owed_to, Some(2));
    }

    #[test]
    fn raise_with_and_priority_and_due() {
        let p = parse_line(
            "Review perf ratings @ Priya !! due: 2026-07-30",
            &people(),
            today(),
        );
        assert_eq!(p.raise_with, Some(3));
        assert_eq!(p.priority, 2);
        assert_eq!(p.due_date, NaiveDate::from_ymd_opt(2026, 7, 30));
        assert_eq!(p.text, "Review perf ratings");
    }

    #[test]
    fn short_date_rolls_forward() {
        // 7/1 is in the past relative to 2026-07-19 → next year.
        let p = parse_line("Ship the thing due 7/1", &people(), today());
        assert_eq!(p.due_date, NaiveDate::from_ymd_opt(2027, 7, 1));
        // 12/1 is upcoming → this year.
        let p = parse_line("Ship the thing due 12/1", &people(), today());
        assert_eq!(p.due_date, NaiveDate::from_ymd_opt(2026, 12, 1));
    }

    #[test]
    fn research_kind() {
        let p = parse_line("? state of the art in eink sync protocols", &people(), today());
        assert_eq!(p.kind, ActionKind::Research);
        assert_eq!(p.text, "state of the art in eink sync protocols");
    }

    #[test]
    fn explicit_kind_prefixes() {
        assert_eq!(
            parse_line("D: we will adopt the new oncall rotation", &people(), today()).kind,
            ActionKind::Decision
        );
        assert_eq!(
            parse_line("T: infra migration is ahead of schedule", &people(), today()).kind,
            ActionKind::Takeaway
        );
        assert_eq!(
            parse_line("N: room double-booked again", &people(), today()).kind,
            ActionKind::Note
        );
    }

    #[test]
    fn unresolved_name_stays_in_text() {
        let p = parse_line("Fix pipeline → Zork", &people(), today());
        assert!(p.delegated_to.is_none());
        assert!(p.text.contains("Zork"));
    }

    #[test]
    fn combined_markers() {
        let p = parse_line(
            "Prep budget deck → AC ◎ Priya @ Bob ! due: 2026-08-01",
            &people(),
            today(),
        );
        assert_eq!(p.delegated_to, Some(1));
        assert_eq!(p.owed_to, Some(3));
        assert_eq!(p.raise_with, Some(2));
        assert_eq!(p.priority, 1);
        assert_eq!(p.due_date, NaiveDate::from_ymd_opt(2026, 8, 1));
        assert_eq!(p.text, "Prep budget deck");
    }
}
