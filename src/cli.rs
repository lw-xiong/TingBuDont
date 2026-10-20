//! Command-line interface: argument parsing, dispatch, default output naming.
//!
//! Default behavior: write a Markdown file beside the input PDF, named after the
//! input stem (foo.pdf -> foo.md). Never overwrite an existing file without
//! explicit permission. `--stdout` writes to stdout instead. `-o` names the
//! output path. `--force` permits overwriting an existing path.

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(
    name = "tingbudont",
    version,
    about = "Convert messy Chinese university PDFs into clean Markdown for downstream LLMs."
)]
pub struct Cli {
    /// Path to the input PDF.
    pub input: PathBuf,

    /// Write Markdown to this path instead of beside the input.
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Write to stdout instead of a file.
    #[arg(long = "stdout")]
    pub stdout: bool,

    /// Allow overwriting an existing output file.
    #[arg(long = "force")]
    pub force: bool,

    /// Emit extraction diagnostics (page count, OCR-required pages, dropped
    /// artifacts) to stderr. Does not affect the Markdown output.
    #[arg(long = "verbose")]
    pub verbose: bool,

    /// OCR pages that carry no text layer (scans), instead of only reporting
    /// them. Adds ~105 MB of resident memory plus ~100 MB per megapixel of
    /// page image while it runs; text PDFs never load the engine.
    #[arg(long = "ocr")]
    pub ocr: bool,

    /// Directory with `det.rten`, `rec.rten`, `dict.txt`. Defaults to
    /// `$TINGBUDONT_MODELS`, else `~/.local/share/tingbudont/models`.
    /// Run `scripts/fetch-models.sh` to populate it (about 7 MB).
    #[arg(long = "ocr-models", value_name = "DIR")]
    pub ocr_models: Option<PathBuf>,

    /// Longest side, in pixels, the page image is clamped to before OCR.
    /// Memory scales with area, so this is the main memory knob: 1400 saves
    /// about half the memory at no measurable accuracy cost on 200 dpi scans.
    #[arg(long = "ocr-max-side", value_name = "PX", default_value_t = 1400)]
    pub ocr_max_side: u32,
}

pub enum CliError {
    Usage(String),
    Failure(String),
}

pub fn run() -> Result<(), CliError> {
    let args = Cli::parse();

    if !args.input.exists() {
        return Err(CliError::Usage(format!(
            "input file not found: {}",
            args.input.display()
        )));
    }
    if args.input.is_dir() {
        return Err(CliError::Usage(format!(
            "input is a directory, expected a PDF: {}",
            args.input.display()
        )));
    }

    let opts = crate::extract::ExtractOptions {
        verbose: args.verbose,
    };
    let mut doc = crate::extract::extract(&args.input, &opts)
        .map_err(|e| CliError::Failure(format!("extraction failed: {e}")))?;
    if args.ocr {
        let models = args
            .ocr_models
            .clone()
            .unwrap_or_else(crate::ocr::default_models_dir);
        let mut engine =
            crate::ocr::Ocr::new(&models, args.ocr_max_side).map_err(CliError::Failure)?;
        let filled = engine
            .fill(&mut doc, &args.input, args.verbose)
            .map_err(|e| CliError::Failure(format!("OCR failed: {e}")))?;
        if args.verbose {
            eprintln!("tingbudont: OCR recovered text on {filled} page(s)");
        }
    }
    crate::layout::reconstruct(&mut doc, &crate::layout::LayoutOptions::default());
    let content = crate::markdown::render(&doc);

    if args.verbose {
        eprintln!(
            "tingbudont: {} page(s), {} block(s), {} OCR-required page(s)",
            doc.page_count(),
            doc.block_count(),
            doc.ocr_required_pages().len()
        );
        for p in doc.ocr_required_pages() {
            // Humans count pages from 1; the model counts from 0.
            eprintln!("tingbudont: OCR required for page {}", p + 1);
        }
    }

    if args.stdout {
        print!("{content}");
        return Ok(());
    }

    let out_path = match &args.output {
        Some(p) => p.clone(),
        None => default_output_path(&args.input),
    };
    crate::output::write_markdown(&out_path, &content, args.force)
        .map_err(|e| CliError::Failure(format!("could not write {}: {e}", out_path.display())))?;
    if args.verbose {
        eprintln!("tingbudont: wrote {}", out_path.display());
    }
    Ok(())
}

/// Default output path: beside the input, same stem, `.md` extension.
/// `foo.pdf` -> `foo.md`; `archive.tar.gz` -> `archive.tar.md`.
pub fn default_output_path(input: &std::path::Path) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| std::path::Path::new(""));
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_string());
    parent.join(format!("{stem}.md"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_output_beside_input() {
        let p = default_output_path(std::path::Path::new("/tmp/lecture_slides.pdf"));
        assert_eq!(p, PathBuf::from("/tmp/lecture_slides.md"));
    }

    #[test]
    fn default_output_preserves_parent() {
        let p = default_output_path(std::path::Path::new("/home/u/notes/ch1.pdf"));
        assert_eq!(p, PathBuf::from("/home/u/notes/ch1.md"));
    }

    #[test]
    fn default_output_no_parent() {
        let p = default_output_path(std::path::Path::new("doc.pdf"));
        assert_eq!(p, PathBuf::from("doc.md"));
    }
}
