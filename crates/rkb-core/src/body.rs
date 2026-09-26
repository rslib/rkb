use std::sync::LazyLock;

use regex::Regex;

/// The structure of a lesson body that lint needs. Line numbers are 1-based within the body.
#[derive(Debug, Default)]
pub struct Body {
    pub h1: Vec<(usize, String)>,
    /// Line of the first non-blank line, if any.
    pub first_line: Option<usize>,
    pub sections: Vec<Section>,
    pub fences: Vec<Fence>,
    pub indented_code: Vec<usize>,
    pub links: Vec<Link>,
}

#[derive(Debug)]
pub struct Section {
    pub heading: String,
    pub line: usize,
    pub has_content: bool,
    /// Indices into `Body::fences`.
    pub fences: Vec<usize>,
}

#[derive(Debug)]
pub struct Fence {
    pub line: usize,
    /// First word of the info string, empty when the fence has none.
    pub lang: String,
}

#[derive(Debug)]
pub struct Link {
    pub line: usize,
    pub target: String,
    pub image: bool,
}

static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(!?)\[[^\]]*\]\(\s*<?([^)\s>]+)>?(?:\s+"[^"]*")?\s*\)"#).unwrap());
static CODE_SPAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`+[^`]*`+").unwrap());
static LIST_ITEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^ {0,3}([-*+]|\d+[.)])(\s|$)").unwrap());

fn fence_open(line: &str) -> Option<(char, usize, &str)> {
    let t = line.trim_start_matches(' ');
    if line.len() - t.len() > 3 {
        return None;
    }
    let c = t.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = t.chars().take_while(|x| *x == c).count();
    (n >= 3).then(|| (c, n, t[n..].trim()))
}

fn is_fence_close(line: &str, c: char, n: usize) -> bool {
    let t = line.trim_start_matches(' ');
    line.len() - t.len() <= 3 && t.chars().take_while(|x| *x == c).count() >= n && t.trim_start_matches(c).trim().is_empty()
}

fn heading(line: &str, level: usize) -> Option<&str> {
    let hashes = "#".repeat(level);
    let rest = line.strip_prefix(&hashes)?;
    if rest.starts_with('#') {
        return None;
    }
    if rest.is_empty() {
        return Some("");
    }
    rest.starts_with([' ', '\t']).then(|| rest.trim().trim_end_matches('#').trim())
}

/// The text under an H2 heading, up to the next H2.
pub fn section_text(body_text: &str, heading: &str) -> Option<String> {
    let scan = scan(body_text);
    let i = scan.sections.iter().position(|s| s.heading == heading)?;
    let start = scan.sections[i].line;
    let end = scan.sections.get(i + 1).map_or(usize::MAX, |s| s.line - 1);
    Some(body_text.lines().skip(start).take(end.saturating_sub(start)).collect::<Vec<_>>().join("\n"))
}

pub fn scan(body: &str) -> Body {
    let mut b = Body::default();
    let mut fence: Option<(char, usize)> = None;
    let mut prev_blank = true;
    let mut prev_code = false;
    let mut in_list = false;

    for (i, line) in body.lines().enumerate() {
        let n = i + 1;
        let blank = line.trim().is_empty();
        if !blank && b.first_line.is_none() {
            b.first_line = Some(n);
        }

        if let Some((c, len)) = fence {
            if is_fence_close(line, c, len) {
                fence = None;
            }
            prev_blank = false;
            continue;
        }

        let indented = line.starts_with("    ") || line.starts_with('\t');
        if !blank && indented && !in_list && (prev_blank || prev_code) {
            b.indented_code.push(n);
            prev_code = true;
            prev_blank = false;
            continue;
        }
        prev_code = false;

        if let Some(s) = b.sections.last_mut()
            && !blank
        {
            s.has_content = true;
        }

        if let Some((c, len, info)) = fence_open(line) {
            fence = Some((c, len));
            let lang = info.split_whitespace().next().unwrap_or("").to_string();
            b.fences.push(Fence { line: n, lang });
            if let Some(s) = b.sections.last_mut() {
                s.fences.push(b.fences.len() - 1);
            }
            prev_blank = false;
            continue;
        }

        if let Some(h) = heading(line, 1) {
            b.h1.push((n, h.to_string()));
        } else if let Some(h) = heading(line, 2) {
            b.sections.push(Section { heading: h.to_string(), line: n, has_content: false, fences: vec![] });
        }

        if LIST_ITEM.is_match(line) {
            in_list = true;
        } else if !blank && !indented {
            in_list = false;
        }

        let text = CODE_SPAN.replace_all(line, "");
        for cap in LINK.captures_iter(&text) {
            b.links.push(Link { line: n, target: cap[2].to_string(), image: !cap[1].is_empty() });
        }
        prev_blank = blank;
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_sections_and_content() {
        let b = scan("\n# Title\n\n## Symptom\ntext\n\n## Fix\n\n### Sub\n## Empty\n");
        assert_eq!(b.h1, vec![(2, "Title".to_string())]);
        assert_eq!(b.first_line, Some(2));
        let names: Vec<_> = b.sections.iter().map(|s| (s.heading.as_str(), s.has_content)).collect();
        assert_eq!(names, [("Symptom", true), ("Fix", true), ("Empty", false)]);
    }

    #[test]
    fn fences_with_and_without_lang() {
        let b = scan("## Check\n```sh\n# not a heading\n## nor this\n```\n~~~\nx\n~~~\n");
        assert_eq!(b.fences.len(), 2);
        assert_eq!(b.fences[0].lang, "sh");
        assert_eq!(b.fences[1].lang, "");
        assert_eq!(b.sections.len(), 1);
        assert_eq!(b.sections[0].fences, vec![0, 1]);
        assert!(b.h1.is_empty());
    }

    #[test]
    fn longer_fence_needs_longer_close() {
        let b = scan("````md\n```\n## inside\n```\n````\n## After\n");
        assert_eq!(b.fences.len(), 1);
        assert_eq!(b.sections.len(), 1);
        assert_eq!(b.sections[0].heading, "After");
    }

    #[test]
    fn indented_code_detection() {
        let b = scan("para\n\n    code\n    more\n\npara\n    lazy continuation\n");
        assert_eq!(b.indented_code, vec![3, 4]);
    }

    #[test]
    fn indented_list_continuation_is_not_code() {
        let b = scan("- item\n\n    continued item text\n");
        assert!(b.indented_code.is_empty());
    }

    #[test]
    fn links_outside_code_only() {
        let b = scan("See [a](../a.md) and ![p](x.assets/p.png \"t\").\n`[no](skip.md)`\n```md\n[no](skip2.md)\n```\n");
        let t: Vec<_> = b.links.iter().map(|l| (l.target.as_str(), l.image)).collect();
        assert_eq!(t, [("../a.md", false), ("x.assets/p.png", true)]);
    }
}
