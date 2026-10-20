use tingbudont::extract::{self, ExtractOptions};
use tingbudont::layout::{detect_columns, merge_into_lines};
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let doc = extract::extract(
        std::path::Path::new(&path),
        &ExtractOptions { verbose: false },
    )
    .unwrap();
    for (i, p) in doc.pages.iter().enumerate() {
        let cols = detect_columns(p);
        for (ci, col) in cols.iter().enumerate() {
            let mut cr: Vec<_> = p
                .runs
                .iter()
                .filter(|r| col.contains(r.bbox.cx(), r.bbox.cy()))
                .collect();
            cr.sort_by(|a, b| {
                b.bbox
                    .cy()
                    .partial_cmp(&a.bbox.cy())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let lines = merge_into_lines(&cr, 3.0);
            println!(
                "page {} col {} (x[{:.0},{:.0}]): {} lines",
                i + 1,
                ci,
                col.x0,
                col.x1,
                lines.len()
            );
            for (j, l) in lines.iter().take(6).enumerate() {
                println!(
                    "  L{j}: y={:.0} fw={} text={:?}",
                    l.bbox.cy(),
                    l.is_full_width(p.width),
                    &l.text[..l.text.len().min(45)]
                );
            }
        }
    }
}
