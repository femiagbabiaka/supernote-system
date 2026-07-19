//! Markdown research report → PDF (for syncing back onto the Supernote).
//!
//! Deliberately simple layout: headings, paragraphs, bullet lists, and a
//! trailing sources section. genpdf handles wrapping and pagination.

use std::path::Path;

use anyhow::{Context, Result};
use genpdf::elements::{Break, Paragraph, UnorderedList};
use genpdf::style::Style;
use genpdf::Element;
use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};

use crate::claude::Source;

pub fn render_report(
    out: &Path,
    title: &str,
    markdown: &str,
    sources: &[Source],
    font_dir: Option<&Path>,
    font_name: &str,
) -> Result<()> {
    let font_dir = font_dir.context(
        "no font dir configured (SUPERNOTE_FONT_DIR); cannot render PDF",
    )?;
    let font = genpdf::fonts::from_files(font_dir, font_name, None)
        .with_context(|| format!("loading font {font_name} from {}", font_dir.display()))?;

    let mut doc = genpdf::Document::new(font);
    doc.set_title(title);
    doc.set_minimal_conformance();
    let mut decorator = genpdf::SimplePageDecorator::new();
    decorator.set_margins(14);
    doc.set_page_decorator(decorator);

    doc.push(
        Paragraph::new(title).styled(Style::new().bold().with_font_size(18)),
    );
    doc.push(Break::new(1.0));

    // Walk the markdown, flushing accumulated text at block boundaries.
    let mut text = String::new();
    let mut style = Style::new();
    let mut list_items: Vec<String> = Vec::new();
    let mut in_list = false;

    let flush =
        |doc: &mut genpdf::Document, text: &mut String, style: Style| {
            let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if !t.is_empty() {
                doc.push(Paragraph::new(t).styled(style));
                doc.push(Break::new(0.5));
            }
            text.clear();
        };

    for event in Parser::new(markdown) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                flush(&mut doc, &mut text, style);
                let size = match level {
                    HeadingLevel::H1 => 16,
                    HeadingLevel::H2 => 14,
                    _ => 12,
                };
                style = Style::new().bold().with_font_size(size);
            }
            Event::End(TagEnd::Heading(_)) => {
                flush(&mut doc, &mut text, style);
                style = Style::new();
            }
            Event::Start(Tag::List(_)) => {
                flush(&mut doc, &mut text, style);
                in_list = true;
            }
            Event::End(TagEnd::List(_)) => {
                let mut list = UnorderedList::new();
                for item in list_items.drain(..) {
                    list.push(Paragraph::new(item));
                }
                doc.push(list);
                doc.push(Break::new(0.5));
                in_list = false;
            }
            Event::End(TagEnd::Item) => {
                let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if !t.is_empty() {
                    list_items.push(t);
                }
                text.clear();
            }
            Event::End(TagEnd::Paragraph)
                if !in_list => {
                    flush(&mut doc, &mut text, style);
                }
            Event::Text(t) | Event::Code(t) => text.push_str(&t),
            Event::SoftBreak | Event::HardBreak => text.push(' '),
            _ => {}
        }
    }
    flush(&mut doc, &mut text, style);

    if !sources.is_empty() {
        doc.push(Break::new(1.0));
        doc.push(
            Paragraph::new("Sources")
                .styled(Style::new().bold().with_font_size(14)),
        );
        let mut list = UnorderedList::new();
        for s in sources {
            list.push(Paragraph::new(format!("{} — {}", s.title, s.url)));
        }
        doc.push(list);
    }

    doc.render_to_file(out)
        .with_context(|| format!("writing {}", out.display()))?;
    Ok(())
}
