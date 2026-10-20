use tingbudont::extract::{self, ExtractOptions};
use tingbudont::layout::{self, LayoutOptions};
use tingbudont::types::Block;
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let mut doc = extract::extract(
        std::path::Path::new(&path),
        &ExtractOptions { verbose: false },
    )
    .unwrap();
    layout::reconstruct(&mut doc, &LayoutOptions::default());
    for (i, b) in doc.blocks.iter().enumerate() {
        let s = match b {
            Block::Heading { level, text } => format!("H{level} {}", &text[..text.len().min(50)]),
            Block::Paragraph(t) => format!("P {}", &t[..t.len().min(60)]),
            Block::ListItem {
                depth,
                ordered,
                text,
            } => format!(
                "L{depth}{} {}",
                if *ordered { "1" } else { "-" },
                &text[..text.len().min(50)]
            ),
            _ => format!("{:?}", b),
        };
        println!("{i}: {s}");
    }
}
