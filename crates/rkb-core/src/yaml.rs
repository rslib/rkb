use std::sync::LazyLock;

use regex::Regex;
use serde_norway::Value;

/// Renders a YAML mapping in block style with indented lists, the style people write by hand.
pub fn block(v: &Value) -> String {
    let mut out = String::new();
    emit(&mut out, v, 0);
    out
}

fn emit(out: &mut String, v: &Value, indent: usize) {
    let pad = " ".repeat(indent);
    match v {
        Value::Mapping(m) => {
            for (k, val) in m {
                let key = scalar(k);
                match val {
                    Value::Mapping(inner) if !inner.is_empty() => {
                        out.push_str(&format!("{pad}{key}:\n"));
                        emit(out, val, indent + 2);
                    }
                    Value::Sequence(items) if !items.is_empty() => {
                        out.push_str(&format!("{pad}{key}:\n"));
                        emit(out, val, indent + 2);
                    }
                    _ => out.push_str(&format!("{pad}{key}: {}\n", scalar(val))),
                }
            }
        }
        Value::Sequence(items) => {
            for item in items {
                match item {
                    Value::Mapping(m) if !m.is_empty() => {
                        let mut inner = String::new();
                        emit(&mut inner, item, indent + 2);
                        out.push_str(&format!("{pad}- {}", &inner[indent + 2..]));
                    }
                    Value::Sequence(s) if !s.is_empty() => {
                        out.push_str(&format!("{pad}-\n"));
                        emit(out, item, indent + 2);
                    }
                    _ => out.push_str(&format!("{pad}- {}\n", scalar(item))),
                }
            }
        }
        _ => out.push_str(&format!("{pad}{}\n", scalar(v))),
    }
}

/// One scalar, or `[]` and `{}` for empty collections. Multi-line text becomes a double-quoted string.
fn scalar(v: &Value) -> String {
    match v {
        Value::Sequence(_) => "[]".into(),
        Value::Mapping(_) => "{}".into(),
        Value::String(s) if s.contains('\n') => {
            let escaped = s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\t', "\\t");
            format!("\"{escaped}\"")
        }
        _ => serde_norway::to_string(v).expect("scalar serializes").trim_end().to_string(),
    }
}

// A plain YAML scalar cannot start with `[` or `{`, so a value that does is a flow collection.
static FLOW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^\s*(?:-\s+)*(?:(?:"[^"]*"|'[^']*'|[^\s"'#\[{-][^:]*):\s+)?[\[{]"#).unwrap());
static EMPTY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:\[\s*\]|\{\s*\})\s*(?:#.*)?$").unwrap());

/// 1-based lines of `frontmatter` that hold a non-empty flow collection such as `[a, b]` or `{k: v}`.
pub fn flow_lines(frontmatter: &str) -> Vec<usize> {
    frontmatter.lines().enumerate().filter(|(_, l)| FLOW.is_match(l) && !EMPTY.is_match(l)).map(|(i, _)| i + 1).collect()
}

/// Rewrites a file's frontmatter in block style and keeps the body byte for byte.
/// `None` when the file has no frontmatter or it does not parse.
pub fn fix_frontmatter(text: &str) -> Option<String> {
    let (fm, body, _) = crate::lesson::split(text).ok()?;
    let value: Value = serde_norway::from_str(fm).ok()?;
    Some(format!("---\n{}---\n{body}", block(&value)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_style_with_indented_lists() {
        let src = "schema: 1\nverified: 2026-09-25\nwhen: {hdf5: \"1.12:1.14.2\", gpu: [mi300a, a100], n: \"12\"}\ntags: [a, b]\nempty: []\nlabels: {sensitivity: public}\nhost: [\"tuo[0-9]*\"]\nitems: [{a: 1, b: 2}]\nnote: \"two\\nlines\"\n";
        let v: Value = serde_norway::from_str(src).unwrap();
        let out = block(&v);
        assert_eq!(
            out,
            "schema: 1\nverified: 2026-09-25\nwhen:\n  hdf5: 1.12:1.14.2\n  gpu:\n    - mi300a\n    - a100\n  n: '12'\ntags:\n  - a\n  - b\nempty: []\nlabels:\n  sensitivity: public\nhost:\n  - tuo[0-9]*\nitems:\n  - a: 1\n    b: 2\nnote: \"two\\nlines\"\n"
        );
        let back: Value = serde_norway::from_str(&out).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn finds_flow_lines() {
        let fm = "tags: [a, b]\nlabels: {sensitivity: public}\nempty: []\nnone: {}\nwhen:\n  gpu: [x]\nlist:\n  - [a]\nq: \"[not flow]\"\nplain: a[b]\n";
        assert_eq!(flow_lines(fm), [1, 2, 6, 8]);
    }

    #[test]
    fn fix_keeps_the_body() {
        let text = "---\ntags: [a, b]\n---\n\n# T\n  body [x]  \n";
        assert_eq!(fix_frontmatter(text).unwrap(), "---\ntags:\n  - a\n  - b\n---\n\n# T\n  body [x]  \n");
        assert!(fix_frontmatter("# no frontmatter\n").is_none());
    }
}
