// Debug helper: runs the pipeline on a PDF path given as argv[1] and prints
// page/run/block counts plus the first few blocks, so layout bugs surface.
// Usage: cargo run --release --example debug_dump -- path/to.pdf
use tingbudont::extract::{self, ExtractOptions};
use tingbudont::layout::{self, LayoutOptions};
use tingbudont::markdown;
use tingbudont::types::Block;

fn main() {
    let path = std::env::args().nth(1).expect("usage: debug_dump <pdf>");
    let mut doc = extract::extract(
        std::path::Path::new(&path),
        &ExtractOptions { verbose: true },
    )
    .expect("extract");
    eprintln!("== pages={} ==", doc.page_count());
    for (i, p) in doc.pages.iter().enumerate() {
        eprintln!(
            "page {}: runs={}, w={} h={} image_only={}",
            i + 1,
            p.runs.len(),
            p.width,
            p.height,
            p.image_only
        );
        for (j, r) in p.runs.iter().take(5).enumerate() {
            eprintln!(
                "  run[{}] text={:?} bbox=({},{})-({},{}) size={}",
                j, r.text, r.bbox.x0, r.bbox.y0, r.bbox.x1, r.bbox.y1, r.font_size
            );
        }
    }
    layout::reconstruct(&mut doc, &LayoutOptions::default());
    eprintln!("== blocks={} ==", doc.block_count());
    for (i, b) in doc.blocks.iter().take(10).enumerate() {
        eprintln!("  block[{i}]: {:?}", short(b));
    }
    let md = markdown::render(&doc);
    eprintln!("== md bytes={} ==", md.len());
    eprintln!("---- head ----");
    let head: String = md.chars().take(800).collect();
    eprintln!("{head}");
}

fn short(b: &Block) -> String {
    match b {
        Block::Paragraph(t) => format!("P({})", &t[..t.len().min(60)]),
        Block::Heading { level, text } => format!("H{level}({})", &text[..text.len().min(60)]),
        Block::ListItem {
            depth,
            text,
            ordered,
        } => {
            format!(
                "L{depth}{}({})",
                if *ordered { "1" } else { "-" },
                &text[..text.len().min(60)]
            )
        }
        Block::Caption(t) => format!("C({})", &t[..t.len().min(60)]),
        Block::Table { grid } => format!(
            "T({}x{})",
            grid.len(),
            grid.iter().map(Vec::len).max().unwrap_or(0)
        ),
        _ => format!("{:?}", b),
    }
}
