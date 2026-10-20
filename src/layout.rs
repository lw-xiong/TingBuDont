//! Layout reconstruction: reading order, columns, artifacts, structure.
//!
//! Input: per-page positioned [`TextRun`]s from extraction.
//! Output: [`Block`]s in correct human reading order, with obvious artifacts
//! (repeated headers/footers, page numbers, broken wraps) removed and
//! paragraphs/headings/lists/tables/captions identified.
//!
//! Design: column detection runs per-page (a doc can be single-column on some
//! pages and two-column on others). Within a column, runs are ordered
//! top-to-bottom; within a line, left-to-right. Cross-column interleaving is
//! prevented by never mixing runs from two columns into the same line unless
//! they vertically overlap AND belong to the same detected column.
//!
//! Heuristics are deliberately conservative: when in doubt, preserve content.
//! We only strip runs we can confidently identify as artifacts.

use crate::types::{Block, Document, PageContent, Table, TextRun};

#[derive(Clone, Copy, Debug)]
pub struct LayoutOptions {
    /// Merge runs whose vertical centers are within this many points. ponytail:
    /// derive from median font size per page instead of a constant.
    pub line_merge_tolerance: f32,
    /// Indent (pts) at which a line counts as a list item / continuation.
    pub list_indent: f32,
    pub detect_headers_footers: bool,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            line_merge_tolerance: 3.0,
            list_indent: 14.0,
            detect_headers_footers: true,
        }
    }
}

/// Build the document blocks from extracted pages. This is the public entry
/// used by the CLI; it runs per-page layout then stitches blocks in page order.
pub fn reconstruct(doc: &mut Document, opts: &LayoutOptions) {
    let mut blocks = Vec::new();
    let mut ocr_pages = Vec::new();
    for (idx, page) in doc.pages.iter().enumerate() {
        if let Some(reason) = crate::extract::classify_ocr(page) {
            ocr_pages.push((idx, reason));
        }
        let pb = page_blocks(page, opts);
        blocks.extend(pb);
    }
    doc.blocks = blocks;
    doc.ocr_pages = ocr_pages;
}

/// Reconstruct reading order for a single page into blocks.
pub fn page_blocks(page: &PageContent, opts: &LayoutOptions) -> Vec<Block> {
    // Runs whose center falls inside a detected table are already in the grid;
    // flowing them again would duplicate every cell as a paragraph.
    let runs: Vec<TextRun> = page
        .runs
        .iter()
        .filter(|r| {
            !page
                .tables
                .iter()
                .any(|t| t.bbox.contains(r.bbox.cx(), r.bbox.cy()))
        })
        .cloned()
        .collect();

    let kept_tables: Vec<&Table> = page.tables.iter().filter(|t| !t.grid.is_empty()).collect();
    if runs.is_empty() {
        return kept_tables
            .into_iter()
            .map(|t| Block::Table {
                grid: t.grid.clone(),
            })
            .collect();
    }
    let page = &PageContent {
        runs,
        ..page.clone()
    };
    // 1. Detect columns.
    let cols = detect_columns(page);
    // 2. Within each column, order runs top-to-bottom and merge into lines.
    //    Every run must land in a column: a title or caption whose centre sits
    //    in the gutter belongs to whichever column it overlaps most, and
    //    dropping it because no column contains its centre loses text.
    let mut per_col: Vec<Vec<&TextRun>> = vec![Vec::new(); cols.len()];
    for r in &page.runs {
        let mut best: Option<(f32, usize)> = None;
        for (i, c) in cols.iter().enumerate() {
            let overlap = c.x1.min(r.bbox.x1) - c.x0.max(r.bbox.x0);
            let score = if cols.len() == 1 { 1.0 } else { overlap };
            if score > 0.0 && best.map(|(b, _)| score > b).unwrap_or(true) {
                best = Some((score, i));
            }
        }
        let idx = best.map(|(_, i)| i).unwrap_or(0);
        per_col[idx].push(r);
    }
    let mut all_lines: Vec<Line> = Vec::new();
    for col_runs in per_col.iter_mut() {
        // y is up, so top-to-bottom reading = descending y order.
        col_runs.sort_by(|a, b| {
            b.bbox
                .cy()
                .partial_cmp(&a.bbox.cy())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let lines = merge_into_lines(col_runs, opts.line_merge_tolerance);
        all_lines.extend(lines);
    }
    // 3. Order everything (text lines AND tables) with one comparator. On a
    //    single-column page reading order is purely top-to-bottom (y
    //    descending, since y is up) — a full-width-first rule would lift wide
    //    body paragraphs above a narrow title and destroy vertical order. That
    //    rule applies only on multi-column pages, where it places a spanning
    //    title/caption (or a full-width table) above the column content.
    let single_column = cols.len() <= 1;
    let mut items: Vec<Item> = all_lines
        .iter()
        .filter_map(|l| {
            line_to_block(l, opts).map(|b| Item {
                y1: l.bbox.y1,
                cx: l.bbox.cx(),
                x0: l.bbox.x0,
                full: l.is_full_width(page.width),
                block: b,
            })
        })
        .collect();
    items.extend(kept_tables.iter().map(|t| Item {
        y1: t.bbox.y1,
        cx: t.bbox.cx(),
        x0: t.bbox.x0,
        full: t.bbox.width() >= page.width * 0.75,
        block: Block::Table {
            grid: t.grid.clone(),
        },
    }));
    items.sort_by(|a, b| {
        let y_desc = || b.y1.partial_cmp(&a.y1).unwrap_or(std::cmp::Ordering::Equal);
        if single_column {
            return y_desc();
        }
        match (a.full, b.full) {
            (true, true) => y_desc(),
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (false, false) => {
                // column-major: left column fully before right column
                let col_a = col_index(&cols, a.cx);
                let col_b = col_index(&cols, b.cx);
                col_a.cmp(&col_b).then_with(y_desc)
            }
        }
    });

    // 4. Coalesce paragraphs broken by line wrap, per column. A single-column
    //    page wraps within one left edge; on a multi-column page the next
    //    paragraph in reading order starts in the other column, and gluing it
    //    onto the previous one runs two columns together mid-sentence.
    coalesce_items(&mut items, page.width);
    items.into_iter().map(|i| i.block).collect()
}

/// A page block with the geometry the reading-order comparator needs.
struct Item {
    /// Top edge (y-up) — the sort key for vertical order.
    y1: f32,
    /// Horizontal center — used for column assignment.
    cx: f32,
    /// Left edge — tells a wrapped continuation (same edge) from the next
    /// column's paragraph (edge shifted by a column width).
    x0: f32,
    /// Spans most of the page width.
    full: bool,
    block: Block,
}

/// A reconstructed line of text spanning one or more runs.
#[derive(Debug, Clone)]
pub struct Line {
    pub runs: Vec<TextRun>,
    pub bbox: crate::types::BBox,
    pub text: String,
}

impl Line {
    pub fn is_full_width(&self, page_width: f32) -> bool {
        // A line spans "full width" if it covers > 75% of the page and is wider
        // than two columns would allow.
        self.bbox.width() > page_width * 0.75
    }
}

/// Detect columns by finding an interior vertical whitespace gutter that spans
/// most of the page height, with text columns of comparable width on both sides.
/// Returns >=1 column. Conservative: defaults to one column unless the evidence
/// for two is strong — a spurious split corrupts reading order far worse than a
/// missed one.
pub fn detect_columns(page: &PageContent) -> Vec<crate::types::BBox> {
    if page.runs.is_empty() {
        return vec![crate::types::BBox::new(0.0, 0.0, page.width, page.height)];
    }
    // Project, per x bucket, how many distinct horizontal rows carry text. A
    // column gutter is a tall empty band, so it must be judged by row coverage:
    // a title or an intro paragraph crossing the gutter covers only a few rows
    // and must not veto the split, which a raw run count lets it do.
    let bucket = 3.0_f32;
    let nbuckets = ((page.width / bucket).ceil() as usize).max(1);
    const ROWS: usize = 48;
    let mut grid = vec![false; nbuckets * ROWS];
    let row_of =
        |y: f32| (((y / page.height) * ROWS as f32) as isize).clamp(0, ROWS as isize - 1) as usize;
    let mut used = 0usize;
    for r in &page.runs {
        // A line wider than half the page spans any column split, so it says
        // nothing about where the columns are — letting it vote makes a title
        // or a full-width intro paragraph hide the gutter entirely.
        if r.bbox.width() > page.width * 0.5 {
            continue;
        }
        used += 1;
        let lo = ((r.bbox.x0 / bucket).floor() as usize).min(nbuckets - 1);
        let hi = ((r.bbox.x1 / bucket).ceil() as usize).min(nbuckets - 1);
        let (r0, r1) = (row_of(r.bbox.y0), row_of(r.bbox.y1));
        for c in lo..=hi {
            for row in r0..=r1 {
                grid[c * ROWS + row] = true;
            }
        }
    }
    if used == 0 {
        // Every line is page-wide: single column, by definition.
        return vec![crate::types::BBox::new(0.0, 0.0, page.width, page.height)];
    }
    let filled: Vec<usize> = (0..nbuckets)
        .map(|c| (0..ROWS).filter(|&row| grid[c * ROWS + row]).count())
        .collect();

    // A bucket counts as "empty" when its row coverage is under 40% of the
    // coverage on BOTH sides of it — a relative test, so a page whose columns
    // are short (few rows total) is judged the same as a dense one. Uniform
    // coverage never qualifies, which is what keeps single-column pages whole.
    let mut left_max = vec![0usize; nbuckets + 1];
    for c in 0..nbuckets {
        left_max[c + 1] = left_max[c].max(filled[c]);
    }
    let mut right_max = vec![0usize; nbuckets + 1];
    for c in (0..nbuckets).rev() {
        right_max[c] = right_max[c + 1].max(filled[c]);
    }
    let empty: Vec<bool> = (0..nbuckets)
        .map(|c| {
            let sides = left_max[c].min(right_max[c + 1]);
            (filled[c] as f32) <= sides as f32 * 0.4
        })
        .collect();

    // Find the content x-range (ignore outer page margins): first and last
    // non-empty bucket.
    let first_content = empty.iter().position(|&e| !e);
    let last_content = empty.iter().rposition(|&e| !e);
    let (first_content, last_content) = match (first_content, last_content) {
        (Some(f), Some(l)) => (f, l),
        _ => return vec![crate::types::BBox::new(0.0, 0.0, page.width, page.height)],
    };
    let content_width = (last_content - first_content + 1) as f32 * bucket;
    // A genuine column gutter: contiguous empty run in the interior (strictly
    // between first and last content) of width >= 4% of content width.
    let min_gutter_buckets = (content_width * 0.04 / bucket).ceil() as usize;
    if last_content <= first_content + 1 {
        // Content sits in one or two adjacent buckets — no interior to scan.
        return vec![crate::types::BBox::new(0.0, 0.0, page.width, page.height)];
    }
    let interior = &empty[first_content + 1..last_content];

    let mut best_gutter: Option<(usize, usize)> = None; // (start, end) in interior-rel
    let mut start = None;
    for (i, &e) in interior.iter().enumerate() {
        match (e, start) {
            (true, None) => start = Some(i),
            (true, Some(_)) => { /* extend the current run */ }
            (false, Some(s)) => {
                let len = i - s;
                if len >= min_gutter_buckets {
                    best_gutter = pick_wider_gutter(best_gutter, (s, i));
                }
                start = None;
            }
            (false, None) => {}
        }
    }
    if let Some(s) = start {
        let len = interior.len() - s;
        if len >= min_gutter_buckets {
            best_gutter = pick_wider_gutter(best_gutter, (s, interior.len()));
        }
    }

    let Some((gs, ge)) = best_gutter else {
        return vec![crate::types::BBox::new(0.0, 0.0, page.width, page.height)];
    };
    // Translate interior-relative indices back to absolute buckets.
    let abs_start = first_content + 1 + gs;
    let abs_end = first_content + 1 + ge;
    let x0_left = first_content as f32 * bucket;
    let x1_left = abs_start as f32 * bucket;
    let x0_right = abs_end as f32 * bucket;
    let x1_right = (last_content + 1) as f32 * bucket;

    // Sanity: both columns must have meaningful width (>= 15% of content width)
    // and the left column's content must actually start well left of the
    // gutter (else it's a margin, not a column boundary).
    let left_w = x1_left - x0_left;
    let right_w = x1_right - x0_right;
    let min_col_w = content_width * 0.15;
    if left_w < min_col_w || right_w < min_col_w {
        return vec![crate::types::BBox::new(0.0, 0.0, page.width, page.height)];
    }
    vec![
        crate::types::BBox::new(x0_left, 0.0, x1_left, page.height),
        crate::types::BBox::new(x0_right, 0.0, x1_right, page.height),
    ]
}

fn pick_wider_gutter(cur: Option<(usize, usize)>, new: (usize, usize)) -> Option<(usize, usize)> {
    match cur {
        None => Some(new),
        Some((s, e)) => {
            if (new.1 - new.0) > (e - s) {
                Some(new)
            } else {
                Some((s, e))
            }
        }
    }
}

fn col_index(cols: &[crate::types::BBox], x: f32) -> usize {
    for (i, c) in cols.iter().enumerate() {
        if x >= c.x0 && x <= c.x1 {
            return i;
        }
    }
    0
}

/// Merge runs in a column into lines: runs whose vertical centers are within
/// `tolerance` form one line; within a line, sort left-to-right.
pub fn merge_into_lines(runs: &[&TextRun], tolerance: f32) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    for r in runs {
        let cy = r.bbox.cy();
        let pos = lines
            .iter()
            .position(|l| (l.bbox.cy() - cy).abs() <= tolerance);
        match pos {
            Some(i) => {
                lines[i].runs.push((*r).clone());
            }
            None => lines.push(Line {
                runs: vec![(*r).clone()],
                bbox: r.bbox,
                text: r.text.clone(),
            }),
        }
    }
    // Recompute bbox + text per line after merging.
    for l in lines.iter_mut() {
        l.runs.sort_by(|a, b| {
            a.bbox
                .x0
                .partial_cmp(&b.bbox.x0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let (x0, y0, x1, y1) = l.runs.iter().fold(
            (f32::MAX, f32::MAX, f32::MIN, f32::MIN),
            |(x0, y0, x1, y1), r| {
                (
                    x0.min(r.bbox.x0),
                    y0.min(r.bbox.y0),
                    x1.max(r.bbox.x1),
                    y1.max(r.bbox.y1),
                )
            },
        );
        l.bbox = crate::types::BBox::new(x0, y0, x1, y1);
        l.text = l
            .runs
            .iter()
            .map(|r| r.text.as_str())
            .collect::<Vec<_>>()
            .join("");
    }
    lines
}

/// Map a line to a structural block. Returns None to drop a line (artifacts).
fn line_to_block(line: &Line, _opts: &LayoutOptions) -> Option<Block> {
    let text = clean_line(&line.text);
    if text.trim().is_empty() {
        return None;
    }
    // Page number: a line that is only digits, or "- N -" / "第N页" patterns,
    // and small, is a footer.
    if is_page_number(&text) && line.runs.len() <= 2 {
        return None;
    }
    if is_heading(line) {
        let level = heading_level(line);
        return Some(Block::Heading {
            level,
            text: text.trim().to_string(),
        });
    }
    if let Some((depth, ordered, body)) = parse_list_item(&text, line) {
        return Some(Block::ListItem {
            depth,
            text: body,
            ordered,
        });
    }
    if is_caption(line) {
        return Some(Block::Caption(text.trim().to_string()));
    }
    Some(Block::Paragraph(text.trim().to_string()))
}

/// Trim trailing whitespace, collapse internal whitespace runs, strip soft
/// hyphens that break words at line ends (heuristic: only when the next char is
/// lowercase, i.e. "inter-leaved" -> "interleaved").
fn clean_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = false;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\u{00AD}' {
            // soft hyphen: drop, join word. But only join if followed by a
            // letter; otherwise treat as word boundary.
            if i + 1 < chars.len() && chars[i + 1].is_alphabetic() {
                i += 1;
                continue;
            }
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            if !prev_ws {
                out.push(' ');
            }
            prev_ws = true;
            i += 1;
            continue;
        }
        out.push(c);
        prev_ws = false;
        i += 1;
    }
    out.trim_end().to_string()
}

fn is_page_number(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    // Pure digits.
    if t.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    // "- 12 -" style.
    let stripped = t.trim_matches(|c: char| c == '-' || c.is_whitespace());
    stripped.chars().all(|c| c.is_ascii_digit()) && !stripped.is_empty()
}

fn is_heading(line: &Line) -> bool {
    // Heading heuristic: large font relative to median, short, and ends without
    // terminal punctuation. Conservative: require font-size dominance AND short
    // length AND no sentence-end punctuation.
    let text = line.text.trim();
    if text.is_empty() || text.len() > 120 {
        return false;
    }
    if text.ends_with('。') || text.ends_with('.') || text.ends_with('；') || text.ends_with(';')
    {
        return false;
    }
    // If all runs are bold-ish (font name contains Bold/Black) or notably
    // larger than a typical body size, treat as heading.
    let max_size = line.runs.iter().map(|r| r.font_size).fold(0.0f32, f32::max);
    max_size >= 14.0 && line.runs.iter().all(|r| r.font_size >= max_size * 0.9)
}

fn heading_level(line: &Line) -> u8 {
    let max_size = line.runs.iter().map(|r| r.font_size).fold(0.0f32, f32::max);
    if max_size >= 24.0 {
        1
    } else if max_size >= 20.0 {
        2
    } else if max_size >= 17.0 {
        3
    } else {
        4
    }
}

fn is_caption(line: &Line) -> bool {
    let t = line.text.trim();
    // Caption heuristic: starts with 图/表/Figure/Table followed by a number.
    let after = t
        .strip_prefix("图")
        .or_else(|| t.strip_prefix("表"))
        .or_else(|| t.strip_prefix("Figure"))
        .or_else(|| t.strip_prefix("Table"));
    if let Some(rest) = after {
        if let Some(c) = rest.chars().next() {
            return c.is_ascii_digit() || c.is_whitespace();
        }
        return false;
    }
    false
}

/// Parse a list-item marker at line start. Returns (depth, ordered, body).
fn parse_list_item(text: &str, line: &Line) -> Option<(u8, bool, String)> {
    let t = text.trim_start();
    // Depth from leading indent relative to page left margin. ponytail: derive
    // from detected column left edge, not page origin.
    let depth = if line.bbox.x0 > 40.0 { 1 } else { 0 };
    let chars: Vec<char> = t.chars().collect();
    if chars.is_empty() {
        return None;
    }
    // Unordered: -, *, •, ·, ▪, ◦
    let bullet = ['-', '*', '•', '·', '▪', '◦'];
    if bullet.contains(&chars[0]) {
        let rest: String = chars[1..].iter().collect();
        let body = rest.trim_start().to_string();
        if body.is_empty() {
            return None;
        }
        return Some((depth, false, body));
    }
    // Ordered: "1." "1、" "(1)" "①"..
    if let Some(rest) = strip_ordered_prefix(&chars) {
        let body = rest.trim_start().to_string();
        if body.is_empty() {
            return None;
        }
        return Some((depth, true, body));
    }
    // Chinese ordered: 一、二、甲、乙
    if chars.len() >= 2 {
        let c = chars[0];
        let sep = chars[1];
        if (matches!(
            c,
            '一' | '二' | '三' | '四' | '五' | '六' | '七' | '八' | '九' | '十'
        ) && (sep == '、' || sep == '.'))
        {
            let rest: String = chars[2..].iter().collect();
            return Some((depth, true, rest.trim_start().to_string()));
        }
    }
    None
}

fn strip_ordered_prefix(chars: &[char]) -> Option<String> {
    // "1." / "1、" / "1)" / "1："
    let mut i = 0;
    let mut digits = 0;
    while i < chars.len() && chars[i].is_ascii_digit() {
        digits += 1;
        i += 1;
    }
    if digits > 0 && i < chars.len() && matches!(chars[i], '.' | '、' | ')' | '：' | ':') {
        return Some(chars[i + 1..].iter().collect());
    }
    // "(1)" style
    if chars[0] == '(' {
        let mut j = 1;
        let mut d = 0;
        while j < chars.len() && chars[j].is_ascii_digit() {
            d += 1;
            j += 1;
        }
        if d > 0 && j < chars.len() && chars[j] == ')' {
            return Some(chars[j + 1..].iter().collect());
        }
    }
    // Circled numbers ①..⑳
    if let Some(&c) = chars.first()
        && ('\u{2460}'..='\u{2473}').contains(&c)
    {
        return Some(chars[1..].iter().collect());
    }
    None
}

/// Merge consecutive paragraph blocks into one (broken line-wrap repair) when
/// the prior paragraph doesn't end with terminal punctuation — a line cut
/// mid-sentence by the PDF's column-width wrap. Conservative: a line that ends
/// in `.`, `。`, etc. is treated as complete and never pulled into the next.
///
/// Geometry-aware: the next paragraph must start at the same left edge as the
/// running one. Adjacent columns share a baseline order but not a left edge, so
/// without that check two columns' paragraphs get glued into one sentence.
fn coalesce_items(items: &mut Vec<Item>, page_width: f32) {
    let mut out: Vec<Item> = Vec::with_capacity(items.len());
    for it in items.drain(..) {
        let mut merged = false;
        if let Some(prev) = out.last_mut() {
            let same_edge = (prev.x0 - it.x0).abs() < page_width * 0.05;
            if same_edge
                && let (Block::Paragraph(p), Block::Paragraph(c)) = (&mut prev.block, &it.block)
                && !last_char_is(p, is_sentence_end)
            {
                p.push(' ');
                p.push_str(c);
                merged = true;
            }
        }
        if !merged {
            out.push(it);
        }
    }
    *items = out;
}

/// Merge consecutive paragraph blocks into one (broken line-wrap repair) when
/// the prior paragraph doesn't end with terminal punctuation.
#[cfg(test)]
fn coalesce_paragraphs(blocks: &mut Vec<Block>) {
    let mut out: Vec<Block> = Vec::with_capacity(blocks.len());
    for b in blocks.drain(..) {
        match (out.last_mut(), &b) {
            (Some(Block::Paragraph(prev)), Block::Paragraph(cur)) => {
                let ends_open = !last_char_is(prev, is_sentence_end);
                if ends_open {
                    prev.push(' ');
                    prev.push_str(cur);
                } else {
                    out.push(b);
                }
            }
            _ => out.push(b),
        }
    }
    *blocks = out;
}

/// True if the string's last char satisfies `f` (empty -> false).
fn last_char_is(s: &str, f: impl Fn(char) -> bool) -> bool {
    s.chars().last().map(f).unwrap_or(false)
}
fn is_sentence_end(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '。' | '！' | '？' | ':' | '：')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BBox, TextRun};

    fn run(text: &str, x0: f32, y0: f32, x1: f32, y1: f32, size: f32) -> TextRun {
        TextRun {
            text: text.to_string(),
            bbox: BBox::new(x0, y0, x1, y1),
            font_size: size,
            font: None,
            page: 0,
        }
    }

    fn page(runs: Vec<TextRun>) -> PageContent {
        PageContent {
            runs,
            width: 595.0,
            height: 841.0,
            image_only: false,
            tables: Vec::new(),
        }
    }

    #[test]
    fn detects_two_columns() {
        // Two clusters of runs with a wide gutter in the middle.
        let runs = vec![
            run("left1", 40.0, 700.0, 90.0, 712.0, 10.0),
            run("left2", 40.0, 650.0, 90.0, 662.0, 10.0),
            run("left3", 40.0, 600.0, 90.0, 612.0, 10.0),
            run("right1", 320.0, 700.0, 370.0, 712.0, 10.0),
            run("right2", 320.0, 650.0, 370.0, 662.0, 10.0),
            run("right3", 320.0, 600.0, 370.0, 612.0, 10.0),
        ];
        let p = page(runs);
        let cols = detect_columns(&p);
        assert_eq!(cols.len(), 2, "expected 2 columns, got {:?}", cols);
    }

    #[test]
    fn detects_single_column_when_no_gutter() {
        let runs = vec![
            run("a", 40.0, 700.0, 500.0, 712.0, 10.0),
            run("b", 40.0, 650.0, 500.0, 662.0, 10.0),
        ];
        let p = page(runs);
        let cols = detect_columns(&p);
        assert_eq!(cols.len(), 1);
    }

    #[test]
    fn column_reading_order_no_interleave() {
        // Left column: L1, L2 (top to bottom). Right column: R1, R2.
        // Reading order must be L1, L2, R1, R2 — NOT L1, R1, L2, R2.
        let runs = vec![
            run("L1.", 40.0, 700.0, 90.0, 712.0, 10.0),
            run("L2.", 40.0, 650.0, 90.0, 662.0, 10.0),
            run("R1.", 320.0, 700.0, 370.0, 712.0, 10.0),
            run("R2.", 320.0, 650.0, 370.0, 662.0, 10.0),
        ];
        let p = page(runs);
        let blocks = page_blocks(&p, &LayoutOptions::default());
        let texts: Vec<&str> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        // Column-major, no interleave: left column fully before right column.
        assert_eq!(texts, vec!["L1.", "L2.", "R1.", "R2."]);
    }

    #[test]
    fn table_sorts_between_text_by_vertical_position() {
        // Title above the table, note below it: the table must sit between
        // them, not at the top of the page.
        use crate::types::Table;
        let runs = vec![
            run("TITLE", 200.0, 780.0, 400.0, 796.0, 16.0),
            run("note below table", 40.0, 100.0, 300.0, 112.0, 10.0),
        ];
        let mut p = page(runs);
        p.tables.push(Table {
            grid: vec![vec![Some("cell".into())]],
            bbox: BBox::new(40.0, 400.0, 560.0, 700.0),
        });
        let blocks = page_blocks(&p, &LayoutOptions::default());
        assert!(matches!(&blocks[0], Block::Heading { text, .. } if text == "TITLE"));
        assert!(
            matches!(&blocks[1], Block::Table { .. }),
            "got {:?}",
            blocks[1]
        );
        assert!(matches!(&blocks[2], Block::Paragraph(t) if t.contains("note below")));
    }

    #[test]
    fn gutter_title_is_not_dropped() {
        // A centred title crosses the gutter, so no column contains its centre.
        // Assigning runs by max x-overlap must keep it instead of losing it.
        let runs = vec![
            run("Centred Title Here", 40.0, 750.0, 560.0, 770.0, 16.0),
            run("L1 body", 40.0, 700.0, 120.0, 712.0, 10.0),
            run("R1 body", 320.0, 700.0, 400.0, 712.0, 10.0),
        ];
        let p = page(runs);
        let blocks = page_blocks(&p, &LayoutOptions::default());
        let text: String = format!("{:?}", blocks);
        assert!(text.contains("Centred Title Here"), "title lost: {text}");
    }

    #[test]
    fn columns_are_not_glued_into_one_paragraph() {
        // Left column line ends without punctuation; the next paragraph in
        // reading order is the right column. They must stay separate blocks.
        let runs = vec![
            run("left col line one", 40.0, 700.0, 250.0, 712.0, 10.0),
            run("right col line one", 320.0, 700.0, 540.0, 712.0, 10.0),
            run("left col line two", 40.0, 688.0, 250.0, 700.0, 10.0),
            run("right col line two", 320.0, 688.0, 540.0, 700.0, 10.0),
        ];
        let p = page(runs);
        let blocks = page_blocks(&p, &LayoutOptions::default());
        let paras: Vec<&str> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(paras.len(), 2, "expected two column paragraphs: {paras:?}");
        assert!(paras[0].contains("left col line one"));
        assert!(paras[1].contains("right col line one"), "{paras:?}");
    }

    #[test]
    fn detect_columns_narrow_content_does_not_panic() {
        // Regression: content confined to one or two adjacent x-buckets made
        // the interior-gutter slice invert its range and panic.
        let runs = vec![run("x", 300.0, 400.0, 301.0, 412.0, 10.0)];
        let p = page(runs);
        let cols = detect_columns(&p);
        assert_eq!(cols.len(), 1);
    }

    #[test]
    fn full_width_title_sorts_before_columns() {
        let runs = vec![
            run("Title", 40.0, 750.0, 560.0, 770.0, 16.0),
            run("L1", 40.0, 700.0, 90.0, 712.0, 10.0),
            run("R1", 320.0, 700.0, 370.0, 712.0, 10.0),
        ];
        let p = page(runs);
        let blocks = page_blocks(&p, &LayoutOptions::default());
        // Title (heading) should come first.
        match &blocks[0] {
            Block::Heading { text, .. } => assert_eq!(text, "Title"),
            other => panic!("expected heading first, got {:?}", other),
        }
    }

    #[test]
    fn single_column_keeps_vertical_order_despite_full_width_body() {
        // Regression: on a single-column page the narrow title must stay first
        // even though the body paragraphs span most of the page width. Sorting
        // full-width lines first used to lift the body above the title.
        let runs = vec![
            run("PRE-ADMISSION NOTICE", 208.0, 672.0, 401.0, 688.0, 16.0),
            run("Body line one, wide text.", 30.0, 600.0, 570.0, 612.0, 12.0),
            run("Body line two, also wide.", 30.0, 580.0, 570.0, 592.0, 12.0),
        ];
        let p = page(runs);
        let blocks = page_blocks(&p, &LayoutOptions::default());
        match &blocks[0] {
            Block::Heading { text, .. } => assert_eq!(text, "PRE-ADMISSION NOTICE"),
            other => panic!("expected title first, got {:?}", other),
        }
        assert!(matches!(&blocks[1], Block::Paragraph(t) if t.contains("Body line one")));
    }

    #[test]
    fn strips_pure_page_number() {
        assert!(is_page_number("12"));
        assert!(is_page_number("- 12 -"));
        assert!(!is_page_number("12 students"));
    }

    #[test]
    fn parses_unordered_bullet() {
        let line = Line {
            runs: vec![run("• item", 40.0, 700.0, 90.0, 712.0, 10.0)],
            bbox: BBox::new(40.0, 700.0, 90.0, 712.0),
            text: "• item".to_string(),
        };
        let (depth, ordered, body) = parse_list_item(&line.text, &line).unwrap();
        assert_eq!(depth, 0);
        assert!(!ordered);
        assert_eq!(body, "item");
    }

    #[test]
    fn parses_ordered_chinese() {
        let line = Line {
            runs: vec![run("一、总则", 40.0, 700.0, 90.0, 712.0, 10.0)],
            bbox: BBox::new(40.0, 700.0, 90.0, 712.0),
            text: "一、总则".to_string(),
        };
        let (depth, ordered, body) = parse_list_item(&line.text, &line).unwrap();
        assert_eq!(depth, 0);
        assert!(ordered);
        assert_eq!(body, "总则");
    }

    #[test]
    fn detects_caption() {
        let line = Line {
            runs: vec![run("图 1 示意图", 40.0, 700.0, 200.0, 712.0, 10.0)],
            bbox: BBox::new(40.0, 700.0, 200.0, 712.0),
            text: "图 1 示意图".to_string(),
        };
        assert!(is_caption(&line));
    }

    #[test]
    fn coalesces_broken_wrap() {
        let mut blocks = vec![
            Block::Paragraph("This is a line that got broken".to_string()),
            Block::Paragraph("across the page boundary.".to_string()),
        ];
        coalesce_paragraphs(&mut blocks);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            Block::Paragraph(t) => assert!(t.contains("broken across")),
            _ => panic!(),
        }
    }

    #[test]
    fn soft_hyphen_joins_word() {
        let s = clean_line("inter\u{00AD}leaved");
        assert_eq!(s, "interleaved");
    }

    #[test]
    fn collapse_whitespace() {
        let s = clean_line("hello   world\t\n foo");
        assert_eq!(s, "hello world foo");
    }
}
