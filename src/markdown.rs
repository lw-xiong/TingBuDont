//! Markdown rendering from reconstructed [`Block`]s.
//!
//! Renders headings, paragraphs, list items, tables, and captions. Emits an
//! OCR warning block when pages needed OCR. Stays close to the block stream —
//! no speculative reformatting.

use crate::types::{Block, Document};

pub fn render(doc: &Document) -> String {
    let mut out = String::with_capacity(2048);

    // OCR notice: if any page needed OCR, note it up front so the reader knows
    // text may be missing from that page. Group consecutive page numbers into
    // ranges for readability.
    if !doc.ocr_pages.is_empty() {
        out.push_str("<!-- OCR required for the following page(s); text may be missing or garbled there. -->\n");
        // Display 1-indexed page numbers (humans count pages from 1).
        let pages: Vec<usize> = doc.ocr_pages.iter().map(|(p, _)| p + 1).collect();
        out.push_str(&format!("<!-- pages: {} -->\n", format_page_ranges(&pages)));
        out.push('\n');
    }

    let mut prev: Option<&Block> = None;
    for b in &doc.blocks {
        render_block(b, prev, &mut out);
        prev = Some(b);
    }
    out
}

fn render_block(b: &Block, prev: Option<&Block>, out: &mut String) {
    match b {
        Block::PageHeader(_) | Block::PageFooter(_) | Block::Gap => {
            // Dropped artifacts / spacing markers — not rendered.
        }
        Block::Heading { level, text } => {
            let lvl = (*level).clamp(1, 6) as usize;
            if needs_blank_before(prev, false) {
                out.push('\n');
            }
            out.push_str(&"#".repeat(lvl));
            out.push(' ');
            out.push_str(text);
            out.push('\n');
        }
        Block::Paragraph(t) => {
            if needs_blank_before(prev, false) {
                out.push('\n');
            }
            out.push_str(t);
            out.push('\n');
        }
        Block::ListItem {
            depth,
            text,
            ordered,
        } => {
            // Blank line before a list starts, but not between consecutive items.
            let starts_list = !matches!(prev, Some(Block::ListItem { .. }));
            if starts_list && needs_blank_before(prev, false) {
                out.push('\n');
            }
            let indent = "  ".repeat(*depth as usize);
            out.push_str(&indent);
            out.push_str(if *ordered { "1. " } else { "- " });
            out.push_str(text);
            out.push('\n');
        }
        Block::Table { grid } => {
            // Render a GitHub-style table. The grid is rows[rows[cells]];
            // ragged rows are padded to the widest. Empty cells become a blank.
            if needs_blank_before(prev, false) {
                out.push('\n');
            }
            render_table(grid, out);
        }
        Block::Caption(t) => {
            if needs_blank_before(prev, false) {
                out.push('\n');
            }
            out.push('*');
            out.push_str(t);
            out.push('*');
            out.push('\n');
        }
    }
}

fn needs_blank_before(prev: Option<&Block>, _force: bool) -> bool {
    match prev {
        None => false,
        Some(b) => !matches!(b, Block::ListItem { .. }),
    }
}

/// Render a cell grid as a GitHub-style Markdown table. Ragged rows are padded;
/// `None` cells render blank. Pipes inside a cell are escaped so the grid stays
/// parseable downstream.
fn render_table(grid: &[Vec<Option<String>>], out: &mut String) {
    let width = grid.iter().map(Vec::len).max().unwrap_or(0);
    if width == 0 {
        return;
    }
    let cell = |r: &[Option<String>], c: usize| -> String {
        r.get(c)
            .and_then(|v| v.as_deref())
            .unwrap_or("")
            .replace('|', "\\|")
            .replace('\n', " ")
    };
    for (i, row) in grid.iter().enumerate() {
        out.push('|');
        for c in 0..width {
            out.push(' ');
            out.push_str(&cell(row, c));
            out.push_str(" |");
        }
        out.push('\n');
        // Header separator after the first row (the common case for forms and
        // textbook tables alike).
        if i == 0 {
            out.push('|');
            for _ in 0..width {
                out.push_str(" --- |");
            }
            out.push('\n');
        }
    }
}

/// Format a sorted list of page numbers into compact ranges: [1,2,3,5,6,9] ->
/// "1-3, 5-6, 9".
fn format_page_ranges(pages: &[usize]) -> String {
    if pages.is_empty() {
        return String::new();
    }
    let mut sorted: Vec<usize> = pages.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut out = String::new();
    let mut start = sorted[0];
    let mut end = sorted[0];
    for &p in &sorted[1..] {
        if p == end + 1 {
            end = p;
        } else {
            push_range(&mut out, start, end);
            out.push_str(", ");
            start = p;
            end = p;
        }
    }
    push_range(&mut out, start, end);
    out
}

fn push_range(out: &mut String, start: usize, end: usize) {
    if start == end {
        out.push_str(&start.to_string());
    } else {
        out.push_str(&format!("{start}-{end}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Block;

    #[test]
    fn renders_heading_levels() {
        let d = Document {
            blocks: vec![
                Block::Heading {
                    level: 1,
                    text: "Title".into(),
                },
                Block::Heading {
                    level: 3,
                    text: "Section".into(),
                },
            ],
            ..Default::default()
        };
        let s = render(&d);
        assert!(s.contains("# Title"));
        assert!(s.contains("### Section"));
    }

    #[test]
    fn renders_pipe_table_with_header() {
        let d = Document {
            blocks: vec![Block::Table {
                grid: vec![
                    vec![Some("姓名".into()), Some("Name".into())],
                    vec![Some("学号".into()), Some("2026".into())],
                ],
            }],
            ..Default::default()
        };
        let s = render(&d);
        assert!(s.contains("| 姓名 | Name |"), "got: {s}");
        assert!(s.contains("| --- | --- |"), "got: {s}");
        assert!(s.contains("| 学号 | 2026 |"), "got: {s}");
    }

    #[test]
    fn table_pads_ragged_rows_and_escapes_pipes() {
        let d = Document {
            blocks: vec![Block::Table {
                grid: vec![
                    vec![Some("a|b".into())],
                    vec![Some("c".into()), Some("d".into())],
                ],
            }],
            ..Default::default()
        };
        let s = render(&d);
        assert!(s.contains(r"| a\|b |  |"), "got: {s}");
        assert!(s.contains("| c | d |"), "got: {s}");
    }

    #[test]
    fn renders_list_with_indent() {
        let d = Document {
            blocks: vec![
                Block::ListItem {
                    depth: 0,
                    text: "one".into(),
                    ordered: false,
                },
                Block::ListItem {
                    depth: 1,
                    text: "nested".into(),
                    ordered: false,
                },
                Block::ListItem {
                    depth: 0,
                    text: "two".into(),
                    ordered: true,
                },
            ],
            ..Default::default()
        };
        let s = render(&d);
        assert!(s.contains("- one"));
        assert!(s.contains("  - nested"));
        assert!(s.contains("1. two"));
    }

    #[test]
    fn renders_ocr_notice() {
        let d = Document {
            ocr_pages: vec![(0, crate::types::OcrReason::ImageOnly)],
            ..Default::default()
        };
        let s = render(&d);
        assert!(s.contains("OCR required"));
        assert!(s.contains("pages: 1"));
    }

    #[test]
    fn formats_page_ranges() {
        assert_eq!(format_page_ranges(&[1, 2, 3, 5, 6, 9]), "1-3, 5-6, 9");
        assert_eq!(format_page_ranges(&[4]), "4");
        assert_eq!(format_page_ranges(&[]), "");
    }

    #[test]
    fn paragraph_coalesced_into_one_line() {
        let d = Document {
            blocks: vec![Block::Paragraph("hello world".into())],
            ..Default::default()
        };
        let s = render(&d);
        assert!(s.contains("hello world\n"));
    }
}
