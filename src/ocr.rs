//! Optional OCR pass for scanned (image-only) pages, backed by rusto-rs.
//!
//! Runs *after* extraction, so a PDF that already has text pays nothing: no
//! model is loaded, no engine is built, peak RSS stays at the extractor's
//! ~18 MB. When `--ocr` is given the engine is built once and each flagged page
//! is decoded, recognized, and dropped before the next one, so peak memory is
//! one page's worth rather than N pages'.
//!
//! Memory is why `--ocr-max-side` exists. The engine's cost is roughly a
//! ~105 MB floor plus ~100 MB per megapixel of input; a 200 dpi A4 scan is
//! 2200×1700 = 3.7 Mpx, which measured 424 MB. Clamping the long side to
//! 1400 px measured 212 MB (−50 %) at 99 % character recall on a dense CJK
//! textbook page, and 1100 px measured 139 MB (−67 %) at 98.8 %. Default 1400
//! buys the largest saving that costs nothing measurable.
//!
//! OCR output is plain text lines positioned as [`TextRun`]s, so the existing
//! layout stage orders them into columns and paragraphs. No table grid is
//! reconstructed from OCR boxes: for an LLM reader, text that is present and
//! readable beats a grid that is half wrong.

use std::path::{Path, PathBuf};

use rusto::{
    DetectTextResult, ImageSource, InitializeConfig, OcrRunOptions, OptimizationOptions,
    OutputGranularity, PPV6_MODEL_CONFIG, RustO,
};

use crate::types::{BBox, Document, TextRun};

/// Smallest and largest accepted `--ocr-max-side`; below ~320 px CJK stops
/// being legible to the detector, above 4000 px the memory saving is gone.
pub const MIN_SIDE: u32 = 320;
pub const MAX_SIDE: u32 = 4000;

pub struct Ocr {
    engine: RustO,
    max_side: u32,
}

impl Ocr {
    /// Build the engine from a directory holding `det.rten`, `rec.rten`,
    /// `dict.txt`. Fails loudly rather than silently skipping pages.
    pub fn new(models: &Path, max_side: u32) -> Result<Self, String> {
        for name in ["det.rten", "rec.rten", "dict.txt"] {
            if !models.join(name).is_file() {
                return Err(format!(
                    "missing OCR model `{name}` in {} — run scripts/fetch-models.sh",
                    models.display()
                ));
            }
        }
        let cfg = InitializeConfig::from_preset(
            PPV6_MODEL_CONFIG,
            models.join("det.rten"),
            models.join("rec.rten"),
            models.join("dict.txt"),
        );
        let engine = RustO::initialize(cfg).map_err(|e| format!("OCR engine init failed: {e}"))?;
        Ok(Self {
            engine,
            max_side: max_side.clamp(MIN_SIDE, MAX_SIDE),
        })
    }

    /// OCR every page the extractor flagged as image-only, in place. Returns
    /// the number of pages that gained text.
    pub fn fill(&mut self, doc: &mut Document, pdf: &Path, verbose: bool) -> Result<usize, String> {
        let targets: Vec<usize> = doc
            .pages
            .iter()
            .enumerate()
            .filter(|(_, p)| p.image_only && p.runs.is_empty())
            .map(|(i, _)| i)
            .collect();
        if targets.is_empty() {
            return Ok(0);
        }
        let src = lopdf::Document::load(pdf).map_err(|e| format!("re-open for OCR failed: {e}"))?;
        let mut done = 0;
        for idx in targets {
            let Some(jpeg) = page_image(&src, (idx + 1) as u32) else {
                if verbose {
                    eprintln!(
                        "tingbudont: page {} has no decodable JPEG, left as image",
                        idx + 1
                    );
                }
                continue;
            };
            let page = &mut doc.pages[idx];
            let size = (page.width, page.height);
            match self.runs(&jpeg, idx, size) {
                Ok(runs) if !runs.is_empty() => {
                    if verbose {
                        eprintln!("tingbudont: OCR page {}: {} line(s)", idx + 1, runs.len());
                    }
                    page.runs = runs;
                    page.image_only = false;
                    done += 1;
                }
                Ok(_) => {}
                Err(e) => {
                    if verbose {
                        eprintln!("tingbudont: OCR page {} failed: {e}", idx + 1);
                    }
                }
            }
        }
        Ok(done)
    }

    /// Recognize one page image. Boxes come back in the input image's pixel
    /// space (top-down); they are mapped into PDF user space (points, y-up) so
    /// the layout stage can treat them exactly like extracted text.
    pub fn runs(
        &mut self,
        jpeg: &[u8],
        page: usize,
        page_size: (f32, f32),
    ) -> Result<Vec<TextRun>, String> {
        let dims = jpeg_dimensions(jpeg)?;
        let opts = OcrRunOptions {
            output: OutputGranularity::Lines,
            optimization: Some(OptimizationOptions::default().with_target_max_side(self.max_side)),
            ..Default::default()
        };
        let result = self
            .engine
            .detect_text(&ImageSource::Bytes(jpeg.to_vec()), &opts)
            .map_err(|e| e.to_string())?;
        let lines = match result {
            DetectTextResult::Structured(lines) => lines,
            DetectTextResult::Spatial(_) => return Ok(Vec::new()),
        };
        let raw: Vec<Line> = lines
            .into_iter()
            .map(|l| Line {
                text: l.text,
                top: l.frame.top,
                left: l.frame.left,
                width: l.frame.width,
                height: l.frame.height,
            })
            .collect();
        Ok(map_lines(&raw, dims, page_size, page))
    }
}

/// A recognized line in image pixel space (origin top-left, y-down).
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    pub top: f32,
    pub left: f32,
    pub width: f32,
    pub height: f32,
}

/// Map pixel-space lines to PDF user space. Pure, so it is tested without the
/// 6 MB of model weights.
pub fn map_lines(
    lines: &[Line],
    image: (u32, u32),
    page_size: (f32, f32),
    page: usize,
) -> Vec<TextRun> {
    let (iw, ih) = image;
    let (pw, ph) = page_size;
    if iw == 0 || ih == 0 || pw <= 0.0 || ph <= 0.0 {
        return Vec::new();
    }
    let sx = pw / iw as f32;
    let sy = ph / ih as f32;
    lines
        .iter()
        .filter_map(|l| {
            let text = l.text.trim();
            if text.is_empty() {
                return None;
            }
            let x0 = l.left * sx;
            let x1 = (l.left + l.width) * sx;
            // y is down in the image, up in PDF space.
            let y1 = ph - l.top * sy;
            let y0 = ph - (l.top + l.height) * sy;
            let bbox = BBox::new(x0, y0, x1, y1);
            Some(TextRun {
                text: text.to_string(),
                font_size: bbox.height().max(1.0),
                bbox,
                font: None,
                page,
            })
        })
        .collect()
}

/// Initialise the OCR directory: `$TINGBUDONT_MODELS`, else the XDG data dir.
pub fn default_models_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("TINGBUDONT_MODELS") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("tingbudont/models")
}

/// Largest decodable JPEG on a page. Scans are one big XObject; logos and
/// inline masks are smaller, so size picks the page image without heuristics.
fn page_image(doc: &lopdf::Document, page_no: u32) -> Option<Vec<u8>> {
    let page_id = *doc.get_pages().get(&page_no)?;
    let resources = resolve(
        doc,
        doc.get_dictionary(page_id).ok()?.get(b"Resources").ok()?,
    )?;
    let xobjects = resolve(doc, resources.get(b"XObject").ok()?)?;
    let mut best: Option<(usize, Vec<u8>)> = None;
    for (_, value) in xobjects.iter() {
        let lopdf::Object::Reference(id) = value else {
            continue;
        };
        let Ok(stream) = doc.get_object(*id).and_then(lopdf::Object::as_stream) else {
            continue;
        };
        if stream
            .dict
            .get(b"Subtype")
            .ok()
            .and_then(|o| o.as_name().ok())
            != Some(b"Image")
        {
            continue;
        }
        if !is_jpeg(&stream.dict) {
            continue;
        }
        let bytes = stream.content.clone();
        if best.as_ref().is_none_or(|(len, _)| bytes.len() > *len) {
            best = Some((bytes.len(), bytes));
        }
    }
    best.map(|(_, bytes)| bytes)
}

fn resolve<'a>(doc: &'a lopdf::Document, obj: &'a lopdf::Object) -> Option<&'a lopdf::Dictionary> {
    match obj {
        lopdf::Object::Reference(id) => doc.get_dictionary(*id).ok(),
        other => other.as_dict().ok(),
    }
}

fn is_jpeg(dict: &lopdf::Dictionary) -> bool {
    let Ok(filter) = dict.get(b"Filter") else {
        return false;
    };
    let is_dct = |o: &lopdf::Object| o.as_name().map(|n| n == b"DCTDecode").unwrap_or(false);
    match filter {
        lopdf::Object::Name(_) => is_dct(filter),
        lopdf::Object::Array(items) => items.iter().any(is_dct),
        _ => false,
    }
}

/// Pixel dimensions from the JPEG header alone — no full decode.
fn jpeg_dimensions(bytes: &[u8]) -> Result<(u32, u32), String> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("image header read failed: {e}"))?
        .into_dimensions()
        .map_err(|e| format!("image dimensions failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, top: f32, left: f32, width: f32, height: f32) -> Line {
        Line {
            text: text.to_string(),
            top,
            left,
            width,
            height,
        }
    }

    #[test]
    fn pixel_boxes_map_to_pdf_points_and_flip_y() {
        // 1000×500 px image on a 500×250 pt page: scale 0.5, y flipped.
        let runs = map_lines(
            &[
                line("top", 0.0, 100.0, 200.0, 20.0),
                line("bottom", 480.0, 0.0, 100.0, 20.0),
            ],
            (1000, 500),
            (500.0, 250.0),
            3,
        );
        assert_eq!(runs.len(), 2);
        // Top of the image is the high-y edge of the page.
        assert!((runs[0].bbox.y1 - 250.0).abs() < 0.01);
        assert!((runs[0].bbox.x0 - 50.0).abs() < 0.01);
        assert!((runs[0].bbox.x1 - 150.0).abs() < 0.01);
        // Bottom line sits below it.
        assert!((runs[1].bbox.y1 - 10.0).abs() < 0.01);
        assert!(runs[1].bbox.y1 < runs[0].bbox.y1);
        assert_eq!(runs[0].page, 3);
        assert!((runs[0].font_size - 10.0).abs() < 0.01);
    }

    #[test]
    fn blank_and_degenerate_input_yields_nothing() {
        assert!(
            map_lines(
                &[line("   ", 0.0, 0.0, 1.0, 1.0)],
                (10, 10),
                (10.0, 10.0),
                0
            )
            .is_empty()
        );
        assert!(map_lines(&[line("x", 0.0, 0.0, 1.0, 1.0)], (0, 10), (10.0, 10.0), 0).is_empty());
    }

    #[test]
    fn max_side_is_clamped() {
        assert_eq!(10u32.clamp(MIN_SIDE, MAX_SIDE), MIN_SIDE);
        assert_eq!(99_999u32.clamp(MIN_SIDE, MAX_SIDE), MAX_SIDE);
    }

    #[test]
    fn missing_models_point_at_the_fetch_script() {
        let err = match Ocr::new(Path::new("/nonexistent-tingbudont-models"), 1400) {
            Ok(_) => panic!("expected an error for a missing model directory"),
            Err(e) => e,
        };
        assert!(err.contains("fetch-models.sh"), "unhelpful message: {err}");
    }
}
