//! PDF text + layout extraction. Library adapter over pdfsink-rs.
//!
//! pdfsink-rs (pure Rust, MIT) parses the PDF and yields, per page, a `Vec<Char>`
//! where each `Char` carries `text` plus positioned geometry (`x0,x1,y0,y1`,
//! `top,bottom`, `width,height`, `size`, `fontname`) and the page also carries
//! `images`, `lines`, `rects`. We map that into our own [`PageContent`] /
//! [`TextRun`] model so the layout stage is independent of the backend.
//!
//! Key mapping decisions:
//! - We use pdfsink's PDF-user-space `y0/y1` (origin bottom-left, y-up), NOT the
//!   `top/bottom` (top-down) convention, because our layout code is written in
//!   y-up space. Both are available on every Char.
//! - A `TextRun` groups consecutive chars on the same line (within a vertical
//!   tolerance) so the layout stage has fewer, larger units to order. We do
//!   NOT merge across the backend's own gaps that would alter reading order.
//! - Image-only pages (chars empty, images non-empty) are flagged for OCR.
//! - Garbled text (bad ToUnicode / PUA) is detected downstream.

use std::path::Path;

use crate::types::{BBox, Document, OcrReason, PageContent, Table, TextRun};

#[derive(Clone, Copy, Debug)]
pub struct ExtractOptions {
    pub verbose: bool,
}

#[derive(Debug)]
pub struct ExtractError(pub String);

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for ExtractError {}

/// Extract a PDF into a [`Document`] with positioned runs per page.
pub fn extract(input: &Path, opts: &ExtractOptions) -> Result<Document, ExtractError> {
    if !input.exists() {
        return Err(ExtractError(format!("file not found: {}", input.display())));
    }
    // Sniff the PDF header before invoking the parser — a clear error message
    // beats a deep parse panic on a corrupt/foreign file.
    let head = std::fs::read(input).map_err(|e| ExtractError(format!("read failed: {e}")))?;
    if head.len() < 5 || &head[..5] != b"%PDF-" {
        return Err(ExtractError(format!(
            "not a PDF (missing %PDF- header): {}",
            input.display()
        )));
    }

    let doc = pdfsink_rs::PdfDocument::open(input)
        .map_err(|e| ExtractError(format!("pdfsink parse error: {e}")))?;

    let mut pages = Vec::with_capacity(doc.len());
    for (idx, page) in doc.pages().iter().enumerate() {
        let runs = chars_to_runs(&page.chars, idx);
        let image_only = page.chars.is_empty() && !page.images.is_empty();
        // Ruled-table extraction: pdfsink's table finder needs drawn lines, so
        // it returns nothing on free-form text pages. A parse failure is
        // non-fatal — the page still flows through the text pipeline.
        // The backend reports boxes in top-down space; we convert to our y-up
        // space so layout can test run containment with one convention.
        let tables = match page.find_tables(pdfsink_rs::TableSettings::default()) {
            Ok(found) => found
                .into_iter()
                .filter_map(|t| {
                    let grid = t.extract(page, &Default::default());
                    // Drop grids with no extractable text at all: on scanned or
                    // image-only pages the finder still reports boxes, and an
                    // all-blank grid renders as a useless empty Markdown table.
                    if grid.is_empty() || grid_is_blank(&grid) {
                        return None;
                    }
                    let b = t.bbox;
                    Some(Table {
                        grid,
                        bbox: BBox::new(
                            b.x0 as f32,
                            (page.height - b.bottom) as f32,
                            b.x1 as f32,
                            (page.height - b.top) as f32,
                        ),
                    })
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        pages.push(PageContent {
            runs,
            width: page.width as f32,
            height: page.height as f32,
            image_only,
            tables,
        });
        if opts.verbose {
            eprintln!(
                "tingbudont: page {} — {} char(s), {} image(s), {} table(s), {}x{} pts",
                idx + 1,
                page.chars.len(),
                page.images.len(),
                pages[idx].tables.len(),
                page.width,
                page.height
            );
        }
    }

    // pdfsink recovers from a per-page parse panic by returning that page empty,
    // so a page can silently lose all its text (unsupported Type0 encodings such
    // as UniGB-UCS2-H are the common cause). Blank pages and image-only pages
    // are legitimate, so only when a page with no text, no image and no table is
    // seen do we go looking for the unsupported font encodings to blame.
    let textless: Vec<usize> = pages
        .iter()
        .enumerate()
        .filter(|(_, p)| p.runs.is_empty() && !p.image_only && p.tables.is_empty())
        .map(|(i, _)| i)
        .collect();
    if !textless.is_empty() {
        let encodings = unsupported_encodings(input);
        if !encodings.is_empty() {
            let list = encodings.join(", ");
            if textless.len() == pages.len() {
                return Err(ExtractError(format!(
                    "no text extracted: unsupported font encoding(s) {list}. \
                     Text extraction cannot decode these; nothing was written."
                )));
            }
            if opts.verbose {
                eprintln!(
                    "tingbudont: {} page(s) produced no text ({:?}); unsupported encoding(s): {list}",
                    textless.len(),
                    textless.iter().map(|p| p + 1).collect::<Vec<_>>()
                );
            }
        }
    }

    Ok(Document {
        pages,
        source: Some(input.to_string_lossy().into_owned()),
        ..Default::default()
    })
}

/// Type0 font encodings pdfsink-rs can decode. Anything else makes it panic
/// internally, which it converts into an empty page.
const DECODABLE_ENCODINGS: [&str; 2] = ["Identity-H", "Identity-V"];

/// Names of Type0 encodings in the file that the text backend cannot decode.
/// Cheap enough to call only when extraction came back suspiciously empty.
fn unsupported_encodings(path: &Path) -> Vec<String> {
    let Ok(doc) = lopdf::Document::load(path) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for obj in doc.objects.values() {
        let Ok(dict) = obj.as_dict() else { continue };
        let is_type0 = dict
            .get(b"Subtype")
            .and_then(lopdf::Object::as_name)
            .map(|n| n == b"Type0")
            .unwrap_or(false);
        if !is_type0 {
            continue;
        }
        let Ok(enc) = dict.get(b"Encoding").and_then(lopdf::Object::as_name) else {
            continue; // stream-embedded CMap: the backend parses those itself
        };
        let name = String::from_utf8_lossy(enc).into_owned();
        if !DECODABLE_ENCODINGS.contains(&name.as_str()) && !out.contains(&name) {
            out.push(name);
        }
    }
    out.sort();
    out
}

/// Group a page's `Char`s into `TextRun`s: chars whose vertical centers are
/// within a tolerance and whose x-order is contiguous form one run. This keeps
/// the runs coarse enough for fast layout ordering but fine enough to preserve
/// intra-line structure. Tolerance derives from the median char size so it
/// adapts to body text vs. headings without a magic constant.
fn chars_to_runs(chars: &[pdfsink_rs::Char], page: usize) -> Vec<TextRun> {
    if chars.is_empty() {
        return Vec::new();
    }
    let median = median_size(chars);
    let tol = (median * 0.5).max(2.0);

    // Sort top-to-bottom in pdfsink top-down space (top ascending), then
    // left-to-right (x0 ascending) within a line.
    let mut order: Vec<usize> = (0..chars.len()).collect();
    order.sort_by(|&a, &b| {
        let ta = chars[a].top;
        let tb = chars[b].top;
        ta.partial_cmp(&tb)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                chars[a]
                    .x0
                    .partial_cmp(&chars[b].x0)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    let mut runs: Vec<TextRun> = Vec::new();
    let mut cur_text = String::new();
    let mut cur_x0 = f32::MAX;
    let mut cur_y0 = f32::MAX;
    let mut cur_x1 = f32::MIN;
    let mut cur_y1 = f32::MIN;
    let mut cur_size = 0.0f32;
    let mut cur_font: Option<String> = None;
    let mut cur_top = 0.0f64;
    let mut started = false;

    let flush = |runs: &mut Vec<TextRun>,
                 text: &mut String,
                 x0: &mut f32,
                 y0: &mut f32,
                 x1: &mut f32,
                 y1: &mut f32,
                 size: &mut f32,
                 font: &mut Option<String>,
                 started: &mut bool| {
        if *started && !text.is_empty() {
            runs.push(TextRun {
                text: std::mem::take(text),
                bbox: BBox::new(*x0, *y0, *x1, *y1),
                font_size: *size,
                font: font.clone(),
                page,
            });
        }
        *started = false;
        *x0 = f32::MAX;
        *y0 = f32::MAX;
        *x1 = f32::MIN;
        *y1 = f32::MIN;
        *size = 0.0;
        *font = None;
    };

    let mut last_x1: f64 = 0.0;
    // A run breaks when the horizontal gap from the previous char exceeds this;
    // a gap like the one between a right-aligned date and a left-aligned name
    // (tens of points) must not be merged into one run.
    let x_gap_limit = (median * 3.0).max(8.0) as f64;

    for &i in &order {
        let c = &chars[i];
        // Decide whether this char continues the current run: same line (top
        // proximity), same font family, AND no large x-gap (which signals a
        // separate text fragment on the same baseline — e.g. a date and a name
        // on the same row).
        let x_gap = if c.x0 >= last_x1 { c.x0 - last_x1 } else { 0.0 };
        let continues = started
            && (c.top - cur_top).abs() <= tol as f64
            && same_font(c.fontname.as_str(), cur_font.as_deref())
            && x_gap <= x_gap_limit;

        if !continues {
            flush(
                &mut runs,
                &mut cur_text,
                &mut cur_x0,
                &mut cur_y0,
                &mut cur_x1,
                &mut cur_y1,
                &mut cur_size,
                &mut cur_font,
                &mut started,
            );
            started = true;
            cur_top = c.top;
        } else {
            // extend
        }

        cur_text.push_str(&c.text);
        cur_x0 = cur_x0.min(c.x0 as f32);
        cur_y0 = cur_y0.min(c.y0 as f32);
        cur_x1 = cur_x1.max(c.x1 as f32);
        cur_y1 = cur_y1.max(c.y1 as f32);
        // Track the largest size in the run for heading detection.
        if (c.size as f32) > cur_size {
            cur_size = c.size as f32;
        }
        if cur_font.is_none() {
            cur_font = Some(c.fontname.clone());
        }
        last_x1 = c.x1;
    }
    flush(
        &mut runs,
        &mut cur_text,
        &mut cur_x0,
        &mut cur_y0,
        &mut cur_x1,
        &mut cur_y1,
        &mut cur_size,
        &mut cur_font,
        &mut started,
    );
    runs
}

/// Treat font names as "same" for run-merging ignoring the Bold/Italic suffix
/// variation, which otherwise fragments a line into many runs. We still capture
/// boldness via the size/font elsewhere for heading detection.
fn same_font(a: &str, b: Option<&str>) -> bool {
    match b {
        None => true,
        Some(b) => base_font(a) == base_font(b),
    }
}

fn base_font(f: &str) -> &str {
    // Strip common weight/style suffixes for grouping.
    let lower = f.to_lowercase();
    for suffix in ["-bold", "-bolditalic", "-italic", "+bold", "+italic"] {
        if lower.ends_with(suffix) {
            return &f[..f.len() - suffix.len()];
        }
    }
    f
}

/// True when every cell in the grid is absent or whitespace-only.
fn grid_is_blank(grid: &[Vec<Option<String>>]) -> bool {
    grid.iter().all(|row| {
        row.iter()
            .all(|c| c.as_deref().is_none_or(|s| s.trim().is_empty()))
    })
}

fn median_size(chars: &[pdfsink_rs::Char]) -> f32 {
    let mut sizes: Vec<f64> = chars.iter().map(|c| c.size).collect();
    sizes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if sizes.is_empty() {
        return 10.0;
    }
    sizes[sizes.len() / 2] as f32
}

/// Heuristic: classify a page as OCR-required.
///
/// A page needs OCR when it has no extractable text but carries an image
/// (scanned/image-only), or when the extracted text is garbled (bad encoding /
/// missing ToUnicode). Garbled detection is conservative: it flags only when a
/// large fraction of glyphs are unprintable or replacement chars.
pub fn classify_ocr(page: &PageContent) -> Option<OcrReason> {
    if page.image_only && page.runs.is_empty() {
        return Some(OcrReason::ImageOnly);
    }
    if page.runs.is_empty() {
        return None; // blank page, no image — not OCR-worthy
    }
    if is_garbled(page) {
        return Some(OcrReason::Garbled);
    }
    None
}

/// Conservative garbled-text detector: flag only if more than 40% of non-space
/// glyphs are control/replacement, AND the page has meaningful text.
fn is_garbled(page: &PageContent) -> bool {
    let mut total = 0usize;
    let mut bad = 0usize;
    for r in &page.runs {
        for ch in r.text.chars() {
            if ch.is_whitespace() {
                continue;
            }
            total += 1;
            if ch == '\u{FFFD}' || ch.is_control() || (is_private_use(ch) && !is_known_pua(ch)) {
                bad += 1;
            }
        }
    }
    if total < 10 {
        return false;
    }
    bad * 10 > total * 4
}

/// Some PDFs deliberately map glyphs into the Private Use Area with a custom
/// ToUnicode. Treat known-good PUA blocks (mathematical) as not-garbled.
fn is_known_pua(_ch: char) -> bool {
    false // conservative: PUA counts as suspicious unless we recognize it.
}

/// Detect Private Use Area codepoints without the unstable `char::is_private_use`.
fn is_private_use(ch: char) -> bool {
    let c = ch as u32;
    (0xE000..=0xF8FF).contains(&c)
        || (0xF0000..=0xFFFFD).contains(&c)
        || (0x100000..=0x10FFFD).contains(&c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BBox, TextRun};

    fn page(runs: Vec<TextRun>) -> PageContent {
        PageContent {
            runs,
            width: 595.0,
            height: 841.0,
            image_only: false,
            tables: Vec::new(),
        }
    }

    fn run(text: &str) -> TextRun {
        TextRun {
            text: text.to_string(),
            bbox: BBox::new(0.0, 0.0, 10.0, 10.0),
            font_size: 10.0,
            font: None,
            page: 0,
        }
    }

    #[test]
    fn classify_image_only_needs_ocr() {
        let p = PageContent {
            runs: vec![],
            width: 595.0,
            height: 841.0,
            image_only: true,
            tables: Vec::new(),
        };
        assert_eq!(classify_ocr(&p), Some(OcrReason::ImageOnly));
    }

    #[test]
    fn classify_blank_page_no_ocr() {
        let p = PageContent {
            runs: vec![],
            width: 595.0,
            height: 841.0,
            image_only: false,
            tables: Vec::new(),
        };
        assert_eq!(classify_ocr(&p), None);
    }

    #[test]
    fn classify_clean_text_no_ocr() {
        let p = page(vec![run("这是中文测试 hello")]);
        assert_eq!(classify_ocr(&p), None);
    }

    #[test]
    fn classify_garbled_text_needs_ocr() {
        // Need >= 10 non-space glyphs for the garbled detector to engage.
        let p = page(vec![run(
            "\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}αβγδε",
        )]);
        assert_eq!(classify_ocr(&p), Some(OcrReason::Garbled));
    }

    #[test]
    fn classify_short_text_not_flagged() {
        let p = page(vec![run("\u{FFFD}")]);
        assert_eq!(classify_ocr(&p), None);
    }

    #[test]
    fn extract_rejects_non_pdf() {
        let tmp = std::env::temp_dir().join("tingbudont_notpdf.txt");
        std::fs::write(&tmp, "hello not a pdf").unwrap();
        let err = extract(&tmp, &ExtractOptions { verbose: false }).unwrap_err();
        assert!(err.0.contains("not a PDF"));
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn extract_rejects_missing_file() {
        let p = std::path::Path::new("/nonexistent/tingbudont_missing.pdf");
        let err = extract(p, &ExtractOptions { verbose: false }).unwrap_err();
        assert!(err.0.contains("not found"));
    }

    #[test]
    fn base_font_strips_weight_suffix() {
        assert_eq!(base_font("SimSun-Bold"), "SimSun");
        assert_eq!(base_font("Helvetica"), "Helvetica");
    }

    #[test]
    fn same_font_handles_none() {
        assert!(same_font("Arial", None));
        assert!(same_font("Arial", Some("Arial")));
        assert!(!same_font("Arial", Some("Times")));
        // bold variant of same family should still group
        assert!(same_font("SimSun-Bold", Some("SimSun")));
    }

    #[test]
    fn blank_grid_is_rejected() {
        // Scanned pages yield table boxes with no text; those must not reach
        // Markdown as empty tables.
        assert!(grid_is_blank(&[vec![None, Some("  ".into())]]));
        assert!(!grid_is_blank(&[vec![None, Some("细胞".into())]]));
    }

    #[test]
    fn unsupported_encodings_survives_unreadable_input() {
        // Missing path and non-PDF bytes must not panic the diagnostic path.
        assert!(unsupported_encodings(std::path::Path::new("/nonexistent/x.pdf")).is_empty());
        let tmp = std::env::temp_dir().join("tingbudont_enc_probe.pdf");
        std::fs::write(&tmp, b"not really a pdf").unwrap();
        assert!(unsupported_encodings(&tmp).is_empty());
        let _ = std::fs::remove_file(&tmp);
    }
}
