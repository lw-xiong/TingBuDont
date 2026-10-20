use tingbudont::extract::{self, ExtractOptions};
use tingbudont::layout::detect_columns;
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let doc = extract::extract(
        std::path::Path::new(&path),
        &ExtractOptions { verbose: false },
    )
    .unwrap();
    for (i, p) in doc.pages.iter().enumerate() {
        let cols = detect_columns(p);
        println!(
            "page {}: {} cols -> {:?}",
            i + 1,
            cols.len(),
            cols.iter().map(|c| (c.x0, c.x1)).collect::<Vec<_>>()
        );
    }
}
