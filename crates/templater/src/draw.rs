//! Template PNG rendering for the Manta (1920×2560 grayscale).
//! Geometry comes from `supernote_core::template_spec` so the webapp can tell
//! the vision model exactly where each printed zone sits.

use std::path::Path;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use anyhow::{Context, Result};
use chrono::DateTime;
use image::{GrayImage, Luma};
use imageproc::drawing::{draw_filled_rect_mut, draw_hollow_rect_mut, draw_text_mut};
use imageproc::rect::Rect;

use supernote_core::template_spec::{TemplateSpec, MARGIN, MAX_CARRIED, PAGE_H, PAGE_W, TICKBOX};

use crate::MeetingTemplateData;

const BLACK: Luma<u8> = Luma([0u8]);
const GRAY: Luma<u8> = Luma([110u8]);
const WHITE: Luma<u8> = Luma([255u8]);

pub struct Fonts {
    pub regular: FontVec,
    pub bold: FontVec,
}

impl Fonts {
    pub fn load(dir: &Path, name: &str) -> Result<Fonts> {
        let load = |file: String| -> Result<FontVec> {
            let path = dir.join(&file);
            let bytes =
                std::fs::read(&path).with_context(|| format!("reading font {}", path.display()))?;
            FontVec::try_from_vec(bytes).with_context(|| format!("parsing font {file}"))
        };
        Ok(Fonts {
            regular: load(format!("{name}-Regular.ttf"))?,
            bold: load(format!("{name}-Bold.ttf"))?,
        })
    }
}

fn text_width(font: &FontVec, scale: PxScale, text: &str) -> f32 {
    let sf = font.as_scaled(scale);
    text.chars().map(|c| sf.h_advance(sf.glyph_id(c))).sum()
}

/// Truncate `text` (appending …) so it fits in `max` pixels.
fn fit(font: &FontVec, scale: PxScale, text: &str, max: f32) -> String {
    if text_width(font, scale, text) <= max {
        return text.to_string();
    }
    let mut s: String = text.to_string();
    while !s.is_empty() && text_width(font, scale, &format!("{s}…")) > max {
        s.pop();
    }
    format!("{s}…")
}

pub fn render_template(meeting: &MeetingTemplateData, fonts: &Fonts) -> Result<GrayImage> {
    let carried_ids: Vec<i64> = meeting.carried.iter().map(|c| c.action_id).collect();
    let spec = TemplateSpec::new(&carried_ids);
    let mut img = GrayImage::from_pixel(PAGE_W, PAGE_H, WHITE);

    // ---- Header band -----------------------------------------------------
    let title_scale = PxScale::from(64.0);
    let title = fit(
        &fonts.bold,
        title_scale,
        &meeting.title,
        (PAGE_W - 2 * MARGIN) as f32,
    );
    draw_text_mut(&mut img, BLACK, MARGIN as i32, 36, title_scale, &fonts.bold, &title);

    let when = match (
        DateTime::parse_from_rfc3339(&meeting.start_time),
        DateTime::parse_from_rfc3339(&meeting.end_time),
    ) {
        (Ok(s), Ok(e)) => format!(
            "{} · {}–{}",
            s.format("%A %-d %B %Y"),
            s.format("%H:%M"),
            e.format("%H:%M")
        ),
        _ => meeting.start_time.clone(),
    };
    draw_text_mut(&mut img, BLACK, MARGIN as i32, 122, PxScale::from(40.0), &fonts.regular, &when);

    let mut meta = Vec::new();
    if let Some(s) = &meeting.series_title {
        if s != &meeting.title {
            meta.push(s.clone());
        }
    }
    if let Some(a) = &meeting.area {
        meta.push(a.clone());
    }
    if !meta.is_empty() {
        draw_text_mut(
            &mut img,
            GRAY,
            MARGIN as i32,
            180,
            PxScale::from(34.0),
            &fonts.regular,
            &meta.join("  ·  "),
        );
    }
    // Header rule.
    draw_filled_rect_mut(
        &mut img,
        Rect::at(MARGIN as i32, spec.header.h as i32 - 6).of_size(PAGE_W - 2 * MARGIN, 3),
        BLACK,
    );

    // ---- Carried-over strip ----------------------------------------------
    let row_scale = PxScale::from(38.0);
    let id_scale = PxScale::from(28.0);
    for (row, carried) in spec.carried_rows.iter().zip(&meeting.carried) {
        // Tick-box.
        draw_hollow_rect_mut(
            &mut img,
            Rect::at(row.tickbox.x as i32, row.tickbox.y as i32)
                .of_size(row.tickbox.w, row.tickbox.h),
            BLACK,
        );
        draw_hollow_rect_mut(
            &mut img,
            Rect::at(row.tickbox.x as i32 + 1, row.tickbox.y as i32 + 1)
                .of_size(row.tickbox.w - 2, row.tickbox.h - 2),
            BLACK,
        );

        // Printed #id at the right edge — the tick-to-action link.
        let id_text = format!("#{}", carried.action_id);
        let id_w = text_width(&fonts.regular, id_scale, &id_text);
        let id_x = (row.rect.x + row.rect.w) as i32 - id_w.ceil() as i32;
        let text_y = row.rect.y as i32 + (row.rect.h as i32 - 38) / 2;
        draw_text_mut(&mut img, GRAY, id_x, text_y + 6, id_scale, &fonts.regular, &id_text);

        // Row text: action text + routing/due markers.
        let mut suffix = String::new();
        if let Some(n) = &carried.delegated_to {
            suffix.push_str(&format!("  → {n}"));
        }
        if let Some(n) = &carried.owed_to {
            // "(o)" is the grammar's ASCII fallback for ◎ — LiberationSans
            // has no U+25CE glyph.
            suffix.push_str(&format!("  (o) {n}"));
        }
        if let Some(n) = &carried.raise_with {
            suffix.push_str(&format!("  @ {n}"));
        }
        if let Some(d) = &carried.due_date {
            suffix.push_str(&format!("  (due {d})"));
        }
        if carried.priority > 0 {
            suffix.push_str("  ");
            suffix.push_str(&"!".repeat(carried.priority.min(3) as usize));
        }
        let text_x = (row.tickbox.x + TICKBOX + 24) as i32;
        let max_w = id_x as f32 - text_x as f32 - 20.0;
        let line = fit(
            &fonts.regular,
            row_scale,
            &format!("{}{suffix}", carried.text),
            max_w,
        );
        draw_text_mut(&mut img, BLACK, text_x, text_y, row_scale, &fonts.regular, &line);
    }

    if meeting.carried.len() > MAX_CARRIED {
        let extra = meeting.carried.len() - MAX_CARRIED;
        draw_text_mut(
            &mut img,
            GRAY,
            MARGIN as i32,
            spec.writing.y as i32 - 26,
            PxScale::from(28.0),
            &fonts.regular,
            &format!("+{extra} more open items — see dashboard"),
        );
    }

    // Separator under the printed zones (start of the freehand area).
    if !spec.carried_rows.is_empty() {
        draw_filled_rect_mut(
            &mut img,
            Rect::at(MARGIN as i32, spec.writing.y as i32 - 4).of_size(PAGE_W - 2 * MARGIN, 2),
            GRAY,
        );
    }

    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CarriedAction;

    /// Renders a representative template. Needs fonts: set SUPERNOTE_FONT_DIR
    /// (e.g. a liberation_ttf truetype dir); skipped otherwise.
    #[test]
    fn renders_sample_template() {
        let Ok(dir) = std::env::var("SUPERNOTE_FONT_DIR") else {
            eprintln!("SUPERNOTE_FONT_DIR unset; skipping render test");
            return;
        };
        let fonts = Fonts::load(Path::new(&dir), "LiberationSans").unwrap();
        let meeting = MeetingTemplateData {
            meeting_id: 1,
            title: "Platform / Infra weekly sync".into(),
            series_title: Some("Infra weekly".into()),
            area: Some("Infrastructure".into()),
            start_time: "2026-07-20T15:00:00+00:00".into(),
            end_time: "2026-07-20T16:00:00+00:00".into(),
            carried: vec![
                CarriedAction {
                    action_id: 42,
                    text: "Draft capacity plan for Q4 GPU fleet expansion".into(),
                    priority: 2,
                    due_date: Some("2026-07-30".into()),
                    delegated_to: Some("Alice Chen".into()),
                    owed_to: None,
                    raise_with: None,
                },
                CarriedAction {
                    action_id: 57,
                    text: "Review incident postmortem".into(),
                    priority: 0,
                    due_date: None,
                    delegated_to: None,
                    owed_to: Some("VP Eng".into()),
                    raise_with: Some("Bob Kowalski".into()),
                },
                CarriedAction {
                    action_id: 63,
                    text: "A very long action item text that should be truncated because it exceeds the row width by a large margin and keeps going and going".into(),
                    priority: 1,
                    due_date: None,
                    delegated_to: None,
                    owed_to: None,
                    raise_with: None,
                },
            ],
        };
        let img = render_template(&meeting, &fonts).unwrap();
        assert_eq!((img.width(), img.height()), (PAGE_W, PAGE_H));
        if let Ok(out) = std::env::var("SUPERNOTE_TEST_OUT") {
            img.save(out).unwrap();
        }
    }
}
