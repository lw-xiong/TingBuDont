# TingBuDont

Chinese university PDFs into Markdown you can paste into an LLM.

One Rust binary, 9.5 MB, no Python and no network. A text PDF costs ~18 MB and
converts in under a second; scanned pages need `--ocr`, which costs ~240 MB while
it runs.

## Usage

```sh
tingbudont lecture_slides.pdf                       # writes lecture_slides.md beside the input
tingbudont textbook_chapter1.pdf -o cleaned_notes.md
tingbudont document.pdf --stdout                    # print instead of writing a file
tingbudont document.pdf --force                     # overwrite an existing output file
tingbudont document.pdf --verbose                   # per-page diagnostics on stderr
tingbudont scanned.pdf --ocr                        # also transcribe scanned pages
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
  ToUnicode / Private Use Area glyphs). With `--ocr` it transcribes them;
  without it, it lists them in an OCR notice.

## OCR (optional)

`--ocr` transcribes image-only pages with [rusto-rs](https://github.com/byrizki/rusto-rs)
(PP-OCRv6 weights on `rten`, pure Rust, no FFI). Text PDFs never load the engine,
so their footprint is unchanged.

```sh
scripts/fetch-models.sh          # ~6 MB into ~/.local/share/tingbudont/models
tingbudont scanned.pdf --ocr --verbose
tingbudont scanned.pdf --ocr --ocr-max-side 1100     # tighter memory budget
TINGBUDONT_MODELS=/path/to/models tingbudont scanned.pdf --ocr
```

Memory is proportional to page area, so `--ocr-max-side` (default 1400 px long
side) is the knob. Medians of 3 runs of the same 3 dense pages from a 200 dpi,
2200×1700 CJK textbook scan, through the release binary (whole PDF, one process):

| long side | peak RSS | time/page | content recall |
|---|---|---|---|
| 2200 (native) | 446 MB | 1.33 s | baseline |
| 1600 | 300 MB | 0.96 s | 98.4 % |
| **1400 (default)** | **240 MB** | **0.82 s** | **98.3 %** |
| 1100 | 170 MB | 0.63 s | 96.3 % |
| 900 | 154 MB | 0.57 s | 89.9 % |

Recall is the character multiset of the page's text against the native-resolution
output, whitespace ignored. Below 1100 px the saving flattens: 900 px buys 16 MB
and 0.06 s/page but loses 6.4 points of text.

Whole-book run of that textbook (152 scanned pages, 67 MB PDF): **about 130 s,
330–350 MB peak, 180 KB of Markdown**. The peak is above the per-page 240 MB
because the page images are pulled from one in-memory copy of the PDF (~130 MB
for this file); that copy is dropped before layout. Text PDFs are unaffected — a
6-page form converts in 18.6 MB, and with `--ocr` given but nothing to OCR it
costs 20.9 MB because the engine is never loaded.

OCR text enters the normal layout pipeline as positioned lines, so columns and
paragraphs are reconstructed the same way as extracted text. Tables on scanned
pages are **not** rebuilt as grids — line boxes are not cell boundaries, and
text that is present reads better to an LLM than a grid that is half wrong.

## Limitations

- **OCR is line-level.** With `--ocr`, scanned pages are transcribed as text
  lines with no font information, so bold/heading detection on those pages falls
  back to size heuristics, and tables come out as flowing text. Decorative
  covers, picture-matching exercises, and vertical text OCR poorly by nature.
  Without `--ocr`, those pages are reported, not transcribed.
- **OCR reads JPEG page images only.** Scans stored as JPEG-2000 or raw
  Flate-compressed bitmaps inside the PDF are reported as image-only and left
  untranscribed (`--verbose` says so).
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

## License

MIT.

