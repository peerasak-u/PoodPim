//! The vocabulary and corrections files as categories, for Settings › คำศัพท์.
//!
//! A file is a preamble of comment lines, then categories: a `# title` line directly above the
//! entries under it. A comment line elsewhere (not directly above an entry) is a note; notes
//! are not kept when Settings writes the file back. Entries before any title form an untitled
//! category.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    pub title: String,
    pub items: Vec<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Doc {
    pub preamble: Vec<String>,
    pub categories: Vec<Category>,
}

/// Which list a file holds: their entries are checked differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// One word or phrase per line.
    Vocabulary,
    /// `heard, heard = wanted` per line.
    Corrections,
}

fn is_entry(line: &str) -> bool {
    let line = line.trim();
    !line.is_empty() && !line.starts_with('#')
}

pub fn parse(text: &str) -> Doc {
    let lines: Vec<&str> = text.lines().collect();
    let mut doc = Doc::default();
    let mut started = false;
    for (i, raw) in lines.iter().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(comment) = line.strip_prefix('#') {
            let title = comment.trim();
            if !title.is_empty() && lines.get(i + 1).is_some_and(|next| is_entry(next)) {
                started = true;
                doc.categories.push(Category { title: title.to_string(), items: Vec::new() });
            } else if !started {
                doc.preamble.push(raw.trim_end().to_string());
            }
            continue;
        }
        started = true;
        if doc.categories.is_empty() {
            doc.categories.push(Category::default());
        }
        doc.categories.last_mut().expect("pushed above").items.push(line.to_string());
    }
    doc
}

/// The file's text: the preamble, then each category after a blank line. Untitled entries come
/// first (after a title they would join that category); empty categories are left out (a title
/// with nothing under it would read back as a note).
pub fn format(doc: &Doc) -> String {
    let mut out = doc.preamble.join("\n");
    let untitled = doc.categories.iter().filter(|c| c.title.is_empty());
    let titled = doc.categories.iter().filter(|c| !c.title.is_empty());
    for c in untitled.chain(titled).filter(|c| !c.items.is_empty()) {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        if !c.title.is_empty() {
            out.push_str(&format!("# {}\n", c.title));
        }
        out.push_str(&c.items.join("\n"));
    }
    out.push('\n');
    out
}

/// One entry as it should be written, or None if it is not one.
fn clean(kind: Kind, item: &str) -> Option<String> {
    let item = item.trim();
    if item.is_empty() || item.starts_with('#') || item.contains('\n') {
        return None;
    }
    match kind {
        Kind::Vocabulary => Some(item.to_string()),
        Kind::Corrections => {
            let (heard, wanted) = item.split_once('=')?;
            let heard: Vec<&str> = heard.split(',').map(str::trim).filter(|h| !h.is_empty()).collect();
            let wanted = wanted.trim();
            (!heard.is_empty() && !wanted.is_empty() && !wanted.contains('=')).then(|| format!("{} = {wanted}", heard.join(", ")))
        }
    }
}

/// Categories sent from Settings, made safe to write: titles on one line without a leading `#`,
/// entries checked for their kind, and no entry twice (vocabulary ignores case, as matching does).
pub fn tidy(kind: Kind, categories: Vec<Category>) -> Vec<Category> {
    let mut seen = std::collections::HashSet::new();
    categories
        .into_iter()
        .map(|c| Category {
            title: c.title.replace(['\n', '\r'], " ").trim().trim_start_matches('#').trim().to_string(),
            items: c
                .items
                .iter()
                .filter_map(|i| clean(kind, i))
                .filter(|i| seen.insert(if kind == Kind::Vocabulary { i.to_lowercase() } else { i.clone() }))
                .collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat(title: &str, items: &[&str]) -> Category {
        Category { title: title.into(), items: items.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn reads_the_default_lists_as_categories() {
        let v = parse(crate::hotwords::DEFAULT_LIST);
        assert!(v.preamble.len() > 5 && v.preamble.iter().all(|l| l.starts_with('#')));
        assert_eq!(v.categories[0].title, "Companies");
        assert!(v.categories.iter().all(|c| !c.title.is_empty() && !c.items.is_empty()));
        let c = parse(crate::corrections::DEFAULT_LIST);
        assert!(c.categories.iter().all(|c| c.items.iter().all(|i| i.contains('='))));
    }

    #[test]
    fn titles_sit_directly_above_entries_and_other_comments_are_notes() {
        let doc = parse("# intro\n#\n# Tools\nGit\n# note about Git\n\n# Words\n# kept? no\nAPI\nSDK\n");
        assert_eq!(doc.preamble, vec!["# intro", "#"]);
        assert_eq!(doc.categories, vec![cat("Tools", &["Git"]), cat("kept? no", &["API", "SDK"])]);
    }

    #[test]
    fn untitled_entries_and_round_trip() {
        let doc = parse("# intro\n\nloose\n# Tools\nGit\n");
        assert_eq!(doc.categories, vec![cat("", &["loose"]), cat("Tools", &["Git"])]);
        let text = format(&doc);
        assert_eq!(text, "# intro\n\nloose\n\n# Tools\nGit\n");
        assert_eq!(parse(&text), doc);
        // The default list survives being written back.
        let v = parse(crate::hotwords::DEFAULT_LIST);
        assert_eq!(parse(&format(&v)), v);
    }

    #[test]
    fn format_puts_untitled_first_and_drops_empty_categories() {
        let doc = Doc { preamble: vec![], categories: vec![cat("A", &["x"]), cat("", &["y"]), cat("Empty", &[])] };
        assert_eq!(format(&doc), "y\n\n# A\nx\n");
    }

    #[test]
    fn tidy_checks_entries_and_drops_repeats() {
        let v = tidy(Kind::Vocabulary, vec![cat("# Tools\n", &[" Git ", "git", "", "# no", "a\nb"]), cat("B", &["GIT", "Rust"])]);
        assert_eq!(v, vec![cat("Tools", &["Git"]), cat("B", &["Rust"])]);
        let c = tidy(Kind::Corrections, vec![cat("", &["ดีพลอย ,ดีพลอยด์= deploy", "no equals", "= x", "a = ", "a = b = c", "ดีพลอย, ดีพลอยด์ = deploy"])]);
        assert_eq!(c, vec![cat("", &["ดีพลอย, ดีพลอยด์ = deploy"])]);
    }
}
