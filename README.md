# TingBuDont

Convert messy Chinese university PDFs into clean Markdown for downstream LLMs.

Pure Rust, single native binary. No Python, no Docker, no network, no external
services. Memory use on typical text PDFs is ~10–20 MB.

## Usage

```sh
tingbudont lecture_slides.pdf                       # writes lecture_slides.md beside the input
tingbudont textbook_chapter1.pdf -o cleaned_notes.md
tingbudont document.pdf --no-header                 # skip the AI context header
tingbudont document.pdf --stdout                    # print instead of writing a file
tingbudont document.pdf --force                     # overwrite an existing output file
tingbudont document.pdf --verbose                   # per-page diagnostics on stderr
```

Default output is `<input stem>.md` next to the input file. An existing output
file is never overwritten unless `--force` is passed.

## What it does

- Extracts Chinese + English text with Unicode, punctuation, numbers, and
  technical terms intact.
- Reconstructs reading order: detects columns by x-density projection and emits
  them column-major, so adjacent columns are never interleaved. On
  single-column pages order is purely top-to-bottom.
- Repairs broken line wraps: a paragraph that does not end in terminal
  punctuation is joined with the next line.
- Classifies blocks as headings, paragraphs, nested lists (ordered/unordered,
  circled-number markers), captions (图/表/Figure/Table), and tables.
- Renders ruled tables (those with drawn cell borders) as GitHub-style Markdown
  grids via pdfsink-rs's table finder. Text inside a detected table is not
  emitted twice.
- Drops repeated page headers/footers and bare page numbers.
- Detects scanned/image-only pages and pages whose text is garbled (bad
  ToUnicode / Private Use Area glyphs), and emits an OCR notice listing those
  pages. It does not perform OCR.
- Prepends a context header instructing the downstream LLM to preserve
  specialized terminology, keep numbers and deadlines exact, and call out
  grading weights, credits, and eligibility conditions.

## Limitations

- **No OCR.** Image-only and garbled pages are reported, not transcribed. Run an
  OCR tool first if you need their text.
- **Tables need drawn borders.** Borderless tables and tables implied only by
  whitespace are not reconstructed as grids; their text flows as paragraphs.
- **Column detection is heuristic.** Very uneven columns, text boxes floating in
  a gutter, or decorative layout can still misorder lines. Multi-column pages
  with a full-width element above a section work; a full-width element *inside*
  a multi-column section is ordered with the columns.
- **No rendered-font boldness.** Heading levels come from font size relative to
  body text, not from the font's weight name, so an unusually large body line
  can be read as a heading.
- **Superscripts/subscripts may be mis-ordered relative to their baseline**
  (e.g. a day number before the "th" suffix) because run grouping follows the
  page's drawing order at the same baseline.
- **Formulas are plain text**, not LaTeX. The original glyphs are preserved.
- **Lists rely on marker recognition.** Bullets/numbering drawn as vector shapes
  rather than text characters are invisible to extraction.

## Build

```sh
cargo build --release
```

Requires stable Rust (edition 2024).
