//! AI context header prepended to Markdown output.
//!
//! Instructs downstream LLMs to preserve terminology, explain concepts, and
//! highlight deadlines, grading weights, and requirements. Omitted with
//! `--no-header`.

use crate::types::Document;

/// The fixed instruction block, tuned for academic/administrative Chinese
/// university documents.
pub fn ai_context_header(_doc: &Document) -> String {
    let mut s = String::new();
    s.push_str(
        "<!-- TingBuDont: AI context header. Keep this when feeding the Markdown to an LLM. -->\n",
    );
    s.push_str("<!-- Source: a Chinese university PDF converted to Markdown. -->\n\n");
    s.push_str(
        "## Instructions for the reading model\n\
\n\
You are reading a Markdown rendering of a Chinese university document \
(lecture slides, textbook chapter, syllabus, transcript, admission/form \
letter, etc.). It was produced by automated PDF text extraction with \
reading-order reconstruction; some layout may be imperfect.\n\
\n\
- Preserve all specialized terminology exactly. Do not translate proper \
nouns, course codes, regulation names, or technical terms (e.g. 学分, \
绩点, 培养方案, JW202, 学号) unless the reader plainly needs a gloss.\n\
- When a term is domain-specific or institution-specific, explain it \
briefly in parentheses on first use: `原词 (plain-language gloss)`.\n\
- Highlight any deadline, date, fee, grading weight, credit requirement, \
or eligibility condition. List these explicitly in a short \"Key \
requirements / deadlines\" section when summarizing.\n\
- Keep numbers, formulas, units, and punctuation exactly as given. \
Percentages, credit hours (学分), GPA (绩点), and money amounts are \
load-bearing.\n\
- If a table was reconstructed, treat rows and columns as approximate; \
re-derive structure from the text if a cell looks misplaced.\n\
- Do not invent content that is not present. If a section looks garbled \
or OCR-flagged, say so rather than guessing the missing text.\n",
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Document;

    #[test]
    fn header_contains_key_directives() {
        let h = ai_context_header(&Document::default());
        assert!(h.contains("deadline"));
        assert!(h.contains("grading"));
        assert!(h.contains("学分"));
        assert!(h.contains("JW202"));
    }
}
