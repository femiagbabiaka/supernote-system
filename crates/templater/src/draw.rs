//! Template PNG rendering for the Manta (1920×2560 grayscale).
//! Geometry comes from `supernote_core::template_spec` so the webapp can tell
//! the vision model exactly where each printed zone sits.

use std::path::Path;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use anyhow::{Context, Result};
use image::{GrayImage, Luma};
use imageproc::drawing::{draw_filled_rect_mut, draw_hollow_rect_mut, draw_text_mut};
use imageproc::rect::Rect;

use supernote_core::template_spec::{
    Rect as Zone, TemplateSpec, MARGIN, MAX_CARRIED, PAGE_H, PAGE_W, TICKBOX,
};

use crate::CarriedAction;

const BLACK: Luma<u8> = Luma([0u8]);
const GRAY: Luma<u8> = Luma([110u8]);
const LIGHT: Luma<u8> = Luma([170u8]);
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

/// What to print on one template.
pub struct TemplateJob<'a> {
    /// Header line labels: ("Meeting:", "With:") for meetings,
    /// ("Title:", "By:") for reading/listening notes.
    pub labels: (&'static str, &'static str),
    /// Pre-printed title (series templates); None leaves the line blank
    /// for handwriting (ad-hoc / reading templates).
    pub title: Option<String>,
    /// Pre-printed second-line value (the 1:1 counterpart); None = blank.
    pub with: Option<String>,
    pub area: Option<String>,
    pub carried: &'a [CarriedAction],
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

/// A labelled write-in ruled line: gray label, baseline rule across the zone,
/// optionally pre-filled with printed text.
fn draw_write_in_line(
    img: &mut GrayImage,
    fonts: &Fonts,
    zone: &Zone,
    label: &str,
    value: Option<&str>,
) {
    let label_scale = PxScale::from(38.0);
    let value_scale = PxScale::from(48.0);
    let baseline = (zone.y + zone.h) as i32 - 14;

    draw_text_mut(
        img,
        GRAY,
        zone.x as i32,
        baseline - 40,
        label_scale,
        &fonts.regular,
        label,
    );
    let label_w = text_width(&fonts.regular, label_scale, label).ceil() as i32 + 24;

    // The rule the user writes on.
    draw_filled_rect_mut(
        img,
        Rect::at(zone.x as i32 + label_w, baseline)
            .of_size(zone.w - label_w as u32, 2),
        LIGHT,
    );

    if let Some(v) = value {
        let v = fit(
            &fonts.bold,
            value_scale,
            v,
            (zone.w - label_w as u32) as f32 - 12.0,
        );
        draw_text_mut(
            img,
            BLACK,
            zone.x as i32 + label_w + 6,
            baseline - 50,
            value_scale,
            &fonts.bold,
            &v,
        );
    }
}

pub fn render_template(job: &TemplateJob, fonts: &Fonts) -> Result<GrayImage> {
    let carried_ids: Vec<i64> = job.carried.iter().map(|c| c.action_id).collect();
    let spec = TemplateSpec::new(&carried_ids);
    let mut img = GrayImage::from_pixel(PAGE_W, PAGE_H, WHITE);

    // ---- Header band: labelled write-in lines -------------------------------
    draw_write_in_line(&mut img, fonts, &spec.title_line, job.labels.0, job.title.as_deref());
    draw_write_in_line(&mut img, fonts, &spec.with_line, job.labels.1, job.with.as_deref());

    if let Some(a) = &job.area {
        let scale = PxScale::from(30.0);
        let w = text_width(&fonts.regular, scale, a);
        draw_text_mut(
            &mut img,
            GRAY,
            (PAGE_W - MARGIN) as i32 - w.ceil() as i32,
            36,
            scale,
            &fonts.regular,
            a,
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
    for (row, carried) in spec.carried_rows.iter().zip(job.carried) {
        // Tick-box (double stroke so it survives e-ink rendering).
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

    if job.carried.len() > MAX_CARRIED {
        let extra = job.carried.len() - MAX_CARRIED;
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

    fn fonts() -> Option<Fonts> {
        let dir = std::env::var("SUPERNOTE_FONT_DIR").ok()?;
        Fonts::load(Path::new(&dir), "LiberationSans").ok()
    }

    #[test]
    fn renders_series_template() {
        let Some(fonts) = fonts() else {
            eprintln!("SUPERNOTE_FONT_DIR unset; skipping render test");
            return;
        };
        let carried = vec![
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
        ];
        let job = TemplateJob {
            labels: ("Meeting:", "With:"),
            title: Some("1:1 Priya".into()),
            with: Some("Priya Natarajan".into()),
            area: Some("Infrastructure".into()),
            carried: &carried,
        };
        let img = render_template(&job, &fonts).unwrap();
        assert_eq!((img.width(), img.height()), (PAGE_W, PAGE_H));
        if let Ok(out) = std::env::var("SUPERNOTE_TEST_OUT") {
            img.save(out).unwrap();
        }
    }

    #[test]
    fn renders_adhoc_template() {
        let Some(fonts) = fonts() else {
            return;
        };
        let job = TemplateJob {
            labels: ("Title:", "By:"),
            title: None,
            with: None,
            area: None,
            carried: &[],
        };
        let img = render_template(&job, &fonts).unwrap();
        assert_eq!((img.width(), img.height()), (PAGE_W, PAGE_H));
        if let Ok(out) = std::env::var("SUPERNOTE_TEST_OUT_ADHOC") {
            img.save(out).unwrap();
        }
    }
}
