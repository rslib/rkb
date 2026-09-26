use crate::body;
use crate::conditions::Version;
use crate::lesson::LessonType;

/// Edit distance where inserting, deleting, replacing or swapping two neighbouring characters costs 1.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

const STOP: [&str; 21] = [
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "in", "is", "it", "of", "on", "or", "that", "the", "to", "was", "with",
];

const INNER: &str = "_./:-@";

fn keep(t: &str) -> bool {
    t.chars().count() >= 2 && !STOP.contains(&t)
}

/// Whether a token is a version such as `1.14.2` or `v1.14`, which search keeps whole.
pub fn is_version(t: &str) -> bool {
    let digit_start =
        t.starts_with(|c: char| c.is_ascii_digit()) || (t.starts_with(['v', 'V']) && t[1..].starts_with(|c: char| c.is_ascii_digit()));
    digit_start && t.contains(['.', '-', '_']) && Version::parse(t).is_some()
}

/// Splits at lower-to-upper case changes: `H5Fopen` gives `H5` and `Fopen`, `HTTPServer` gives `HTTP` and `Server`.
fn camel_parts(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = vec![];
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| chars[j]);
        let next = chars.get(i + 1).copied();
        let boundary = c.is_uppercase()
            && prev.is_some_and(|p| p.is_lowercase() || p.is_ascii_digit() || (p.is_uppercase() && next.is_some_and(char::is_lowercase)));
        if boundary && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Search terms of a text: lowercase, no stemming, versions whole, other compound tokens whole and in parts.
pub fn tokens(text: &str) -> Vec<String> {
    let mut out = vec![];
    each_token(text, &mut |t| out.push(t.to_string()));
    out
}

/// Calls `f` with each search term of `text`, in the same order as `tokens`, without allocating per plain word.
pub fn each_token(text: &str, f: &mut impl FnMut(&str)) {
    let mut buf = String::new();
    for raw in text.split(|c: char| c.is_whitespace() || !(c.is_alphanumeric() || INNER.contains(c))) {
        let raw = raw.trim_matches(|c: char| INNER.contains(c));
        if raw.is_empty() {
            continue;
        }
        // Fast path: a plain ASCII word with at most a leading capital has no parts and is no version.
        let plain = raw.bytes().all(|b| b.is_ascii_alphanumeric()) && !raw.bytes().skip(1).any(|b| b.is_ascii_uppercase());
        if plain {
            buf.clear();
            buf.push_str(raw);
            buf.make_ascii_lowercase();
            if keep(&buf) {
                f(&buf);
            }
            continue;
        }
        if is_version(raw) {
            f(&raw.to_lowercase());
            continue;
        }
        let whole = raw.to_lowercase();
        let parts: Vec<String> =
            raw.split(|c: char| INNER.contains(c)).flat_map(camel_parts).map(|p| p.to_lowercase()).filter(|p| keep(p)).collect();
        if keep(&whole) {
            f(&whole);
        }
        if parts.len() > 1 || parts.first().is_some_and(|p| *p != whole) {
            for p in &parts {
                f(p);
            }
        }
    }
}

/// The section whose first sentence summarizes a lesson of this type.
pub fn lead_heading(kind: LessonType) -> &'static str {
    match kind {
        LessonType::Pitfall => "Symptom",
        LessonType::Recipe => "When to use",
        LessonType::Fact => "Statement",
        LessonType::Decision => "Decision",
        LessonType::Preference => "Rule",
    }
}

/// The first sentence of the lesson's lead section, skipping code fences and list markers.
pub fn summary(kind: LessonType, body_text: &str) -> Option<String> {
    let section = body::section_text(body_text, lead_heading(kind))?;
    let mut in_fence = false;
    for line in section.lines() {
        let t = line.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || t.is_empty() {
            continue;
        }
        let t = t.trim_start_matches(['-', '*', ' ']).trim();
        let end = [". ", "? ", "! "].iter().filter_map(|p| t.find(p)).min().map_or(t.len(), |i| i + 1);
        return Some(t[..end].to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(text: &str, want: &[&str]) {
        let t = tokens(text);
        for w in want {
            assert!(t.iter().any(|x| x == w), "{w} missing from {t:?}");
        }
    }

    #[test]
    fn technical_tokens() {
        has("H5Fopen", &["h5fopen", "h5", "fopen"]);
        has("hdf5::File", &["hdf5::file", "hdf5", "file"]);
        has("snake_case_name", &["snake_case_name", "snake", "case", "name"]);
        has("src/io/writer.cpp", &["src/io/writer.cpp", "src", "io", "writer", "cpp"]);
        has("HTTPServer", &["httpserver", "http", "server"]);
        assert_eq!(tokens("1.14.2"), ["1.14.2"]);
        assert_eq!(tokens("v1.14"), ["v1.14"]);
        assert_eq!(tokens("hdf5"), ["hdf5"]);
        assert!(!tokens("version 1.14.2 here").contains(&"14".to_string()));
        assert_eq!(tokens("The fix is in the file."), ["fix", "file"]);
        assert_eq!(tokens("--start-group"), ["start-group", "start", "group"]);
    }

    #[test]
    fn distance_counts_a_swap_as_one() {
        assert_eq!(edit_distance("cmkae", "cmake"), 1);
        assert_eq!(edit_distance("tuolmne", "tuolumne"), 1);
        assert_eq!(edit_distance("abc", "abc"), 0);
        assert_eq!(edit_distance("", "ab"), 2);
    }

    #[test]
    fn summaries_by_type() {
        let cases = [
            (LessonType::Pitfall, "## Symptom\nConfigure fails with \"Could NOT find HDF5\". It stops.\n"),
            (LessonType::Recipe, "## When to use\n- Before a job writes large files. Then stripe.\n"),
            (LessonType::Fact, "## Statement\n```sh\nx\n```\nSrun binds cores by default! More.\n"),
            (LessonType::Decision, "## Decision\nUse RE2 for per-line parsing.\n"),
            (LessonType::Preference, "## Rule\nWrite one subject line? Yes.\n"),
        ];
        let want = [
            "Configure fails with \"Could NOT find HDF5\".",
            "Before a job writes large files.",
            "Srun binds cores by default!",
            "Use RE2 for per-line parsing.",
            "Write one subject line?",
        ];
        for ((kind, body), w) in cases.iter().zip(want) {
            assert_eq!(summary(*kind, &format!("# T\n\n{body}")).as_deref(), Some(w), "{kind:?}");
        }
        assert_eq!(summary(LessonType::Fact, "# T\n"), None);
    }
}
