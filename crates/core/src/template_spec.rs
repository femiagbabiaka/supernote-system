//! Template geometry for the Manta (A5X2) — 1920×2560 @ 300 PPI.
//!
//! Shared by the templater (which draws these zones onto the PNG) and the
//! webapp (which describes them to the vision model so it knows what is
//! pre-printed where, and which handwriting belongs to which zone).

use serde::{Deserialize, Serialize};

pub const PAGE_W: u32 = 1920;
pub const PAGE_H: u32 = 2560;

/// Left/right page margin in pixels.
pub const MARGIN: u32 = 60;
/// Height of the pre-printed header band.
pub const HEADER_H: u32 = 260;
/// Height of one carried-over action row.
pub const CARRIED_ROW_H: u32 = 84;
/// Side of the square tick-box printed at the left of each carried row.
pub const TICKBOX: u32 = 44;
/// Maximum carried-over rows before we truncate (and print "+N more").
pub const MAX_CARRIED: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// One pre-printed carried-over action row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CarriedRow {
    /// The action id, printed at the right edge as `#id` so a tick can be
    /// traced back to the exact open action.
    pub action_id: i64,
    pub rect: Rect,
    pub tickbox: Rect,
}

/// Full zone layout for one meeting template page.
///
/// The header carries two write-in ruled lines instead of calendar data —
/// there is no calendar integration by design. On a series template the
/// title line is pre-printed with the series name; on the generic ad-hoc
/// template both lines are blank and handwritten.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateSpec {
    pub page_w: u32,
    pub page_h: u32,
    pub header: Rect,
    /// "Meeting:" ruled line — handwritten (or pre-printed) meeting title.
    pub title_line: Rect,
    /// "With:" ruled line — handwritten attendee names.
    pub with_line: Rect,
    pub carried_rows: Vec<CarriedRow>,
    /// Freehand writing area (everything below the printed zones).
    pub writing: Rect,
}

impl TemplateSpec {
    /// Compute the layout for a template carrying the given open action ids.
    /// At most [`MAX_CARRIED`] rows are laid out; callers should note overflow.
    pub fn new(carried_action_ids: &[i64]) -> Self {
        let header = Rect {
            x: 0,
            y: 0,
            w: PAGE_W,
            h: HEADER_H,
        };
        // Two ruled write-in lines inside the header band.
        let title_line = Rect {
            x: MARGIN,
            y: 40,
            w: PAGE_W - 2 * MARGIN,
            h: 80,
        };
        let with_line = Rect {
            x: MARGIN,
            y: 140,
            w: PAGE_W - 2 * MARGIN,
            h: 80,
        };
        let shown = &carried_action_ids[..carried_action_ids.len().min(MAX_CARRIED)];
        let mut carried_rows = Vec::with_capacity(shown.len());
        let mut y = HEADER_H;
        for &action_id in shown {
            let rect = Rect {
                x: MARGIN,
                y,
                w: PAGE_W - 2 * MARGIN,
                h: CARRIED_ROW_H,
            };
            let tickbox = Rect {
                x: MARGIN,
                y: y + (CARRIED_ROW_H - TICKBOX) / 2,
                w: TICKBOX,
                h: TICKBOX,
            };
            carried_rows.push(CarriedRow {
                action_id,
                rect,
                tickbox,
            });
            y += CARRIED_ROW_H;
        }
        // A little breathing room between the printed strip and freehand ink.
        let writing_top = if carried_rows.is_empty() { y } else { y + 30 };
        TemplateSpec {
            page_w: PAGE_W,
            page_h: PAGE_H,
            header,
            title_line,
            with_line,
            carried_rows,
            writing: Rect {
                x: 0,
                y: writing_top,
                w: PAGE_W,
                h: PAGE_H.saturating_sub(writing_top),
            },
        }
    }

    /// JSON form embedded in the vision prompt.
    pub fn to_prompt_json(&self) -> String {
        serde_json::to_string(self).expect("spec serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_stack_below_header() {
        let spec = TemplateSpec::new(&[11, 22, 33]);
        assert_eq!(spec.carried_rows.len(), 3);
        assert_eq!(spec.carried_rows[0].rect.y, HEADER_H);
        assert_eq!(spec.carried_rows[1].rect.y, HEADER_H + CARRIED_ROW_H);
        assert!(spec.writing.y > spec.carried_rows[2].rect.y + CARRIED_ROW_H - 1);
        assert_eq!(spec.writing.h, PAGE_H - spec.writing.y);
    }

    #[test]
    fn truncates_at_max() {
        let ids: Vec<i64> = (0..20).collect();
        let spec = TemplateSpec::new(&ids);
        assert_eq!(spec.carried_rows.len(), MAX_CARRIED);
    }

    #[test]
    fn empty_carried_gives_full_writing_area() {
        let spec = TemplateSpec::new(&[]);
        assert!(spec.carried_rows.is_empty());
        assert_eq!(spec.writing.y, HEADER_H);
    }
}
