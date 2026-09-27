use std::path::Path;

use rkb_core::lesson::{LessonType, Status};
use rkb_core::list::{Entry, Listing, Overview, TopicView};
use serde_json::{Value, json};

use crate::output::{Output, paint};

/// Markers for one character set. ASCII is used when the locale is not UTF-8.
pub(crate) struct Marks {
    pub pitfall: &'static str,
    pub recipe: &'static str,
    pub fact: &'static str,
    pub decision: &'static str,
    pub preference: &'static str,
    pub check: &'static str,
    pub sep: &'static str,
    pub ellipsis: &'static str,
}

pub const UTF8: Marks =
    Marks { pitfall: "!", recipe: "›", fact: "·", decision: "◆", preference: "★", check: "✓", sep: "·", ellipsis: "…" };

pub const ASCII: Marks =
    Marks { pitfall: "!", recipe: ">", fact: "-", decision: "*", preference: "+", check: "ok", sep: "-", ellipsis: "..." };

/// The first of `LC_ALL`, `LC_CTYPE`, `LANG` that is set decides, as in POSIX.
pub fn utf8_from(vars: [Option<String>; 3]) -> bool {
    vars.into_iter().flatten().find(|v| !v.is_empty()).is_some_and(|v| {
        let v = v.to_lowercase();
        v.contains("utf-8") || v.contains("utf8")
    })
}

pub(crate) fn marks() -> &'static Marks {
    let var = |k: &str| std::env::var(k).ok();
    if utf8_from([var("LC_ALL"), var("LC_CTYPE"), var("LANG")]) { &UTF8 } else { &ASCII }
}

pub(crate) fn width() -> usize {
    std::env::var("COLUMNS").ok().and_then(|c| c.parse().ok()).filter(|w| *w >= 20).unwrap_or(80)
}

pub(crate) fn len(s: &str) -> usize {
    s.chars().count()
}

pub(crate) fn cut(s: &str, max: usize, m: &Marks) -> String {
    if len(s) <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(len(m.ellipsis));
    format!("{}{}", s.chars().take(keep).collect::<String>().trim_end(), m.ellipsis)
}

pub(crate) fn kind_name(k: LessonType) -> &'static str {
    match k {
        LessonType::Pitfall => "pitfall",
        LessonType::Recipe => "recipe",
        LessonType::Fact => "fact",
        LessonType::Decision => "decision",
        LessonType::Preference => "preference",
    }
}

pub(crate) fn marker(k: LessonType, m: &Marks) -> (&'static str, &'static str) {
    match k {
        LessonType::Pitfall => (m.pitfall, "31"),
        LessonType::Recipe => (m.recipe, "36"),
        LessonType::Fact => (m.fact, "34"),
        LessonType::Decision => (m.decision, "35"),
        LessonType::Preference => (m.preference, "33"),
    }
}

fn conditions(e: &Entry) -> String {
    e.conditions.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join("  ")
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

const MIN_TITLE: usize = 24;

/// One lesson line: marker, title, then conditions and flags right-aligned at `end`.
struct Line {
    marker: (&'static str, &'static str),
    title: String,
    conds: String,
    flags: Vec<(String, &'static str)>,
    dim: bool,
}

impl Line {
    fn right_len(&self) -> usize {
        let flags: usize = self.flags.iter().map(|(f, _)| len(f) + 2).sum();
        let conds = if self.conds.is_empty() { 0 } else { len(&self.conds) + 2 };
        conds + flags
    }

    fn natural(&self) -> usize {
        4 + len(&self.title) + self.right_len()
    }

    /// The title keeps at least `MIN_TITLE` characters; conditions are cut first when space is short.
    fn render(&self, end: usize, color: bool, m: &Marks) -> String {
        let flags_len: usize = self.flags.iter().map(|(f, _)| len(f) + 2).sum();
        let title_max = end.saturating_sub(4 + self.right_len()).max(len(&self.title).min(MIN_TITLE));
        let title = cut(&self.title, title_max, m);
        let mut right_text = String::new();
        let mut right_plain = String::new();
        if !self.conds.is_empty() {
            let room = end.saturating_sub(4 + len(&title) + flags_len + 2);
            let conds = if room > len(m.ellipsis) { cut(&self.conds, room, m) } else { String::new() };
            if !conds.is_empty() {
                right_plain.push_str(&format!("  {conds}"));
                right_text.push_str(&format!("  {}", paint(color, "2", &conds)));
            }
        }
        for (f, code) in &self.flags {
            right_plain.push_str(&format!("  {f}"));
            right_text.push_str(&format!("  {}", paint(color, code, f)));
        }
        let pad = if right_plain.is_empty() { 0 } else { end.saturating_sub(4 + len(&title) + len(&right_plain)) };
        let line = format!("  {} {title}{}{right_text}", paint(color && !self.dim, self.marker.1, self.marker.0), " ".repeat(pad));
        if self.dim { paint(color, "2", &strip_to_plain(&line, color)) } else { line }
    }
}

/// Dimmed lines are painted once as a whole, so their parts must not carry their own colors.
fn strip_to_plain(line: &str, color: bool) -> String {
    if !color {
        return line.to_string();
    }
    let mut out = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn line_for(e: &Entry, m: &Marks, retired: bool) -> Line {
    let mut flags = vec![];
    if retired {
        let word = if e.status == Status::Archived { "archived" } else { "superseded" };
        flags.push((word.to_string(), "2"));
    } else if e.status == Status::Stale {
        flags.push(("stale".to_string(), "33"));
    }
    if e.checked {
        flags.push((m.check.to_string(), "32"));
    }
    Line { marker: marker(e.kind, m), title: e.title.clone(), conds: conditions(e), flags, dim: retired }
}

fn topic_human(v: &TopicView, color: bool, m: &Marks, width: usize) -> String {
    let mut head = v.folder.clone();
    if let Some(t) = &v.title {
        head.push_str(&format!(" {} {t}", m.sep));
    }
    head.push_str(&format!(" {} {}", m.sep, plural(v.count(), "lesson")));
    let mut out = format!("{}\n", paint(color, "1", &cut(&head, width, m)));

    let groups: Vec<(String, Vec<Line>, bool)> = v
        .groups
        .iter()
        .map(|g| (g.tag.clone().unwrap_or_else(|| "untagged".into()), g.lessons.iter().map(|e| line_for(e, m, false)).collect(), false))
        .chain(
            (!v.retired.is_empty())
                .then(|| (format!("archived {} superseded", m.sep), v.retired.iter().map(|e| line_for(e, m, true)).collect(), true)),
        )
        .collect();
    let end = groups.iter().flat_map(|(_, ls, _)| ls).map(Line::natural).max().unwrap_or(0).min(width);
    for (name, lines, dim) in &groups {
        out.push_str(&format!("\n{}\n", paint(color, if *dim { "2" } else { "1" }, name)));
        for l in lines {
            out.push_str(&l.render(end, color, m));
            out.push('\n');
        }
    }

    let all: Vec<&Entry> = v.groups.iter().flat_map(|g| &g.lessons).chain(&v.retired).collect();
    let mut legend = vec![];
    for k in [LessonType::Pitfall, LessonType::Recipe, LessonType::Fact, LessonType::Decision, LessonType::Preference] {
        if all.iter().any(|e| e.kind == k) {
            legend.push(format!("{} {}", marker(k, m).0, kind_name(k)));
        }
    }
    if all.iter().any(|e| e.checked) {
        legend.push(format!("{} checked", m.check));
    }
    if !legend.is_empty() {
        out.push_str(&format!("\n{}\n", paint(color, "2", &legend.join("  "))));
    }
    if v.unparsed > 0 {
        out.push_str(&format!("\n{} not shown because they do not parse; run `rkb lint`\n", plural(v.unparsed, "lesson file")));
    }
    out
}

/// `name count` cells, wrapped at `width` with `indent` spaces.
fn cells(items: &[(String, usize)], indent: usize, width: usize) -> String {
    let mut out = String::new();
    let mut line = String::new();
    for (name, n) in items {
        let cell = format!("{name} {n}");
        if !line.is_empty() && indent + len(&line) + 3 + len(&cell) > width {
            out.push_str(&format!("{}{line}\n", " ".repeat(indent)));
            line.clear();
        }
        if !line.is_empty() {
            line.push_str("   ");
        }
        line.push_str(&cell);
    }
    if !line.is_empty() {
        out.push_str(&format!("{}{line}\n", " ".repeat(indent)));
    }
    out
}

fn overview_human(ov: &Overview, root: &Path, color: bool, m: &Marks, width: usize) -> String {
    let end = width.min(56);
    let count_line = |label: &str, indent: usize, n: usize, bold: bool| {
        let n = n.to_string();
        let pad = end.saturating_sub(indent + len(label) + len(&n)).max(2);
        format!("{}{}{}{n}\n", " ".repeat(indent), paint(color && bold, "1", label), " ".repeat(pad))
    };
    let mut out = format!("{}\n", paint(color, "1", &format!("{} {} {}", crate::output::tilde(root), m.sep, plural(ov.total, "lesson"))));
    if ov.scopes.is_empty() {
        out.push_str("\nNo lessons yet. Add one with `rkb add --topic <topic>`.\n");
    }
    for scope in &ov.scopes {
        out.push('\n');
        if scope.name == "general" {
            let g = &scope.groups[0];
            out.push_str(&count_line("general", 0, g.lessons(), true));
            let items: Vec<(String, usize)> = g.topics.iter().map(|t| (t.name.clone(), t.lessons)).collect();
            out.push_str(&paint_block(&cells(&items, 2, width), color));
        } else {
            out.push_str(&format!("{}\n", paint(color, "1", &scope.name)));
            for g in &scope.groups {
                out.push_str(&count_line(g.name.as_deref().unwrap_or(""), 2, g.lessons(), false));
                let items: Vec<(String, usize)> = g.topics.iter().map(|t| (t.name.clone(), t.lessons)).collect();
                out.push_str(&paint_block(&cells(&items, 4, width), color));
            }
        }
    }
    let mut foot = vec![];
    if ov.stale > 0 {
        foot.push(format!("{} stale", ov.stale));
    }
    if ov.retired > 0 {
        foot.push(format!("{} archived or superseded", ov.retired));
    }
    if !foot.is_empty() {
        out.push_str(&format!("\n{}\n", paint(color, "2", &foot.join(&format!(" {} ", m.sep)))));
    }
    if ov.unparsed > 0 {
        out.push_str(&format!(
            "\n{} not counted because they do not parse or are misplaced; run `rkb lint`\n",
            plural(ov.unparsed, "lesson file")
        ));
    }
    out
}

fn paint_block(block: &str, color: bool) -> String {
    if !color {
        return block.to_string();
    }
    block.lines().map(|l| format!("{}\n", paint(true, "2", l))).collect()
}

fn row(e: &Entry) -> Value {
    json!({
        "id": e.id,
        "type": kind_name(e.kind),
        "status": e.status.as_str(),
        "title": e.title,
        "tags": e.tags.join(" "),
        "when": e.conditions.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join("; "),
        "verified": e.verified.to_string(),
    })
}

pub fn render(listing: &Listing, root: &Path, color: bool) -> Output {
    let m = marks();
    let w = width();
    match listing {
        Listing::Topic(v) => {
            let rows: Vec<Value> = v.groups.iter().flat_map(|g| &g.lessons).chain(&v.retired).map(row).collect();
            let mut data = json!({ "folder": v.folder, "title": v.title, "lessons": rows });
            let mut help = vec!["Run `rkb show <id>` to read one lesson".to_string()];
            if v.unparsed > 0 {
                data["unparsed"] = json!(v.unparsed);
                help.push("Run `rkb lint` to see why some lesson files do not parse".into());
            }
            data["help"] = json!(help);
            Output { data, human: topic_human(v, color, m, w), exit: 0, raw: false }
        }
        Listing::Overview(ov) => {
            let topics: Vec<Value> = ov
                .scopes
                .iter()
                .flat_map(|s| &s.groups)
                .flat_map(|g| &g.topics)
                .map(|t| json!({ "folder": t.folder, "lessons": t.lessons, "stale": t.stale, "retired": t.retired }))
                .collect();
            let mut data = json!({
                "root": root.display().to_string(),
                "lessons": ov.total,
                "stale": ov.stale,
                "retired": ov.retired,
                "topics": topics,
                "help": ["Run `rkb list <folder>` to see the lessons of one topic"],
            });
            if ov.unparsed > 0 {
                data["unparsed"] = json!(ov.unparsed);
            }
            Output { data, human: overview_human(ov, root, color, m, w), exit: 0, raw: false }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_order() {
        let s = |v: &str| Some(v.to_string());
        assert!(utf8_from([None, None, s("en_US.UTF-8")]));
        assert!(!utf8_from([None, None, s("C")]));
        assert!(!utf8_from([s("C"), None, s("en_US.UTF-8")]));
        assert!(utf8_from([Some(String::new()), s("C.utf8"), s("C")]));
        assert!(!utf8_from([None, None, None]));
    }

    #[test]
    fn cut_and_cells() {
        assert_eq!(cut("abcdef", 4, &UTF8), "abc…");
        assert_eq!(cut("abcdef", 5, &ASCII), "ab...");
        assert_eq!(cut("abc", 5, &UTF8), "abc");
        let items: Vec<(String, usize)> = ["cmake", "cpp", "git", "latex"].iter().map(|n| (n.to_string(), 10)).collect();
        let out = cells(&items, 2, 24);
        assert!(out.lines().all(|l| len(l) <= 24), "{out}");
        assert_eq!(out.lines().count(), 2);
    }
}
