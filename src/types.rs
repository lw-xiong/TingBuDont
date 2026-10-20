//! Document model shared by all pipeline stages.
//!
//! The extraction stage produces a [`Document`] of positioned [`TextRun`]s per
//! page. The layout stage groups runs into [`Block`]s in reading order with
//! structure (paragraph, heading, list, table, caption). The markdown stage
//! serializes blocks to Markdown.
//!
//! Positioning is in PDF user-space points (origin bottom-left, y-up), carried
//! verbatim from the extractor. Layout heuristics are written in that space and
//! never re-derive coordinates they don't have.

use std::path::Path;

/// A point in PDF user space (points, origin bottom-left, y-up).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// A bounding box in PDF user space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BBox {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl BBox {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self {
            x0: x0.min(x1),
            y0: y0.min(y1),
            x1: x0.max(x1),
            y1: y0.max(y1),
        }
    }
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }
    /// Horizontal center.
    pub fn cx(&self) -> f32 {
        (self.x0 + self.x1) * 0.5
    }
    /// Vertical center.
    pub fn cy(&self) -> f32 {
        (self.y0 + self.y1) * 0.5
    }
    /// Does this box's vertical span overlap the other's by more than half?
    pub fn v_overlaps_half(&self, other: &BBox) -> bool {
        let lo = self.y0.max(other.y0);
        let hi = self.y1.min(other.y1);
        hi - lo > self.height().max(other.height()) * 0.5
    }
    /// Is this box entirely above `other` (higher y, since y is up)?
    pub fn strictly_above(&self, other: &BBox) -> bool {
        self.y0 >= other.y1
    }
    /// Does this box contain a point?
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }
}

/// A run of text with a bounding box and font-size estimate.
///
/// `text` may contain multiple glyphs; the extractor decides run granularity.
/// Layout treats a run as one atomic unit for column assignment.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    pub text: String,
    pub bbox: BBox,
    pub font_size: f32,
    /// Font name if known — used to detect bold/heading-like runs.
    pub font: Option<String>,
    pub page: usize,
}

impl TextRun {
    pub fn len_chars(&self) -> usize {
        self.text.chars().count()
    }
    /// Leading whitespace?
    pub fn starts_ws(&self) -> bool {
        self.text.starts_with(char::is_whitespace)
    }
}

/// A page's extracted content.
#[derive(Debug, Clone, Default)]
pub struct PageContent {
    pub runs: Vec<TextRun>,
    pub width: f32,
    pub height: f32,
    /// True if no text was found AND the page appears to contain a raster image
    /// (image-only / scanned). OCR is required to recover text.
    pub image_only: bool,
    /// Tables detected on the page (cell grids from the backend's table
    /// finder). Each grid is `rows[rows[cells]]`; a cell is `None` when the
    /// backend found no text in that cell. Empty for pages with no ruled
    /// tables.
    pub tables: Vec<Table>,
}

/// A detected table: its cell grid plus the region it occupies. `bbox` is in
/// PDF user space (y-up) and lets the layout stage drop text runs already
/// captured in the grid, so cell text is not emitted twice.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub grid: TableGrid,
    pub bbox: BBox,
}

/// A reconstructed table: a 2-D grid of optional cell strings.
pub type TableGrid = Vec<Vec<Option<String>>>;

impl PageContent {
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }
}

/// Why a page is flagged as needing OCR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcrReason {
    /// No text runs and the page has at least one image (scanned/image-only).
    ImageOnly,
    /// Text was extracted but it is garbled (bad encoding/CMap), suggesting a
    /// subset/embedded font with no ToUnicode mapping.
    Garbled,
}

/// Structural block produced by the layout stage.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// A run known to be a header (repeats across pages). Kept for diagnostics;
    /// normally stripped from output.
    PageHeader(TextRun),
    /// A run known to be a footer / page number.
    PageFooter(TextRun),
    /// Heading text. `level` 1..=6 maps to Markdown `#`..`######`.
    Heading {
        level: u8,
        text: String,
    },
    Paragraph(String),
    /// List item; `depth` is 0-based nesting.
    ListItem {
        depth: u8,
        text: String,
        ordered: bool,
    },
    /// A table reconstructed as a 2-D grid of optional cell strings. The grid
    /// comes from the backend's table finder (ruled tables); the markdown
    /// stage renders it as a GitHub-style table.
    Table {
        grid: TableGrid,
    },
    Caption(String),
    /// A horizontal gap large enough to suggest a thematic break, but not a
    /// heading. Rendered as a thematic break only when the gap is unusual.
    Gap,
}

/// The reconstructed document: blocks in reading order plus diagnostics.
#[derive(Debug, Clone, Default)]
pub struct Document {
    pub blocks: Vec<Block>,
    pub pages: Vec<PageContent>,
    pub ocr_pages: Vec<(usize, OcrReason)>,
    pub source: Option<String>,
}

impl Document {
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }
    pub fn ocr_required_pages(&self) -> Vec<usize> {
        self.ocr_pages.iter().map(|(p, _)| *p).collect()
    }
    pub fn empty_with(source: &Path) -> Self {
        Self {
            source: Some(source.to_string_lossy().into_owned()),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bbox_normalizes_corners() {
        let b = BBox::new(10.0, 20.0, 5.0, 25.0);
        assert_eq!(b.x0, 5.0);
        assert_eq!(b.y0, 20.0);
        assert_eq!(b.x1, 10.0);
        assert_eq!(b.y1, 25.0);
        assert!((b.width() - 5.0).abs() < 1e-6);
        assert!((b.height() - 5.0).abs() < 1e-6);
    }

    #[test]
    fn bbox_v_overlaps_half_works() {
        let a = BBox::new(0.0, 0.0, 10.0, 10.0); // height 10
        // overlap with b = 6 (> half of a's height 5)
        let b = BBox::new(0.0, 4.0, 10.0, 14.0);
        assert!(a.v_overlaps_half(&b));
        let c = BBox::new(0.0, 15.0, 10.0, 25.0); // no overlap
        assert!(!a.v_overlaps_half(&c));
    }

    #[test]
    fn strictly_above_uses_y_up() {
        let top = BBox::new(0.0, 80.0, 10.0, 90.0);
        let bottom = BBox::new(0.0, 10.0, 10.0, 20.0);
        assert!(top.strictly_above(&bottom));
        assert!(!bottom.strictly_above(&top));
    }
}
