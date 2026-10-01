use jiff::civil::Date;
use serde::{Deserialize, Serialize};
use serde_norway::Mapping;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LessonType {
    Pitfall,
    Recipe,
    Fact,
    Decision,
    Preference,
}

impl LessonType {
    pub fn from_name(name: &str) -> Option<Self> {
        serde_norway::from_str(name).ok()
    }

    /// The H2 headings a lesson of this type must have, in the required order.
    pub fn required_headings(self) -> &'static [&'static str] {
        match self {
            LessonType::Pitfall => &["Symptom", "Cause", "Fix", "Evidence"],
            LessonType::Recipe => &["When to use", "Steps", "Evidence"],
            LessonType::Fact => &["Statement", "Evidence"],
            LessonType::Decision => &["Context", "Decision", "Why", "Rejected options"],
            LessonType::Preference => &["Rule", "Why", "How to apply"],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Active,
    Stale,
    Superseded,
    Archived,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Active => "active",
            Status::Stale => "stale",
            Status::Superseded => "superseded",
            Status::Archived => "archived",
        }
    }

    /// The extra H2 heading this status requires, if any.
    pub fn required_heading(self) -> Option<&'static str> {
        match self {
            Status::Superseded => Some("Why superseded"),
            Status::Archived => Some("Why archived"),
            Status::Active | Status::Stale => None,
        }
    }

    /// Whether search and README lists show the lesson in the main list.
    pub fn is_current(self) -> bool {
        matches!(self, Status::Active | Status::Stale)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerifiedHow {
    Checked,
    Ran,
    Read,
    Told,
}

/// Lesson frontmatter. Field order here is the order rkb writes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frontmatter {
    pub schema: u32,
    pub id: String,
    #[serde(rename = "type")]
    pub kind: LessonType,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Mapping::is_empty")]
    pub when: Mapping,
    pub verified: Date,
    pub verified_how: VerifiedHow,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Example queries someone would type before they know the lesson exists; only the `bm25-bert` index reads them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queries: Vec<String>,
    #[serde(default, skip_serializing_if = "Mapping::is_empty")]
    pub labels: Mapping,
    #[serde(default, skip_serializing_if = "Mapping::is_empty")]
    pub meta: Mapping,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Lesson {
    /// Path relative to the knowledge base root, with `/` separators.
    pub path: String,
    pub frontmatter: Frontmatter,
    pub body: String,
    /// 1-based line in the file where the body starts.
    pub body_line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// 1-based line in the file, when known.
    pub line: Option<usize>,
    pub message: String,
}

/// Splits a lesson file into frontmatter text, body text and the body's first line.
pub fn split(text: &str) -> Result<(&str, &str, usize), ParseError> {
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
        .ok_or_else(|| ParseError { line: Some(1), message: "file does not start with a `---` frontmatter line".into() })?;
    let fm_start = text.len() - rest.len();
    let mut offset = 0;
    for (i, line) in rest.split_inclusive('\n').enumerate() {
        if line.trim_end_matches(['\n', '\r']) == "---" {
            let fm = &rest[..offset];
            let body = &rest[offset + line.len()..];
            return Ok((fm, body, i + 3));
        }
        offset += line.len();
    }
    Err(ParseError { line: Some(1), message: format!("frontmatter starting at byte {fm_start} has no closing `---` line") })
}

pub fn parse(path: &str, text: &str) -> Result<Lesson, ParseError> {
    let (fm, body, body_line) = split(text)?;
    let frontmatter: Frontmatter = serde_norway::from_str(fm)
        .map_err(|e| ParseError { line: e.location().map(|l| l.line() + 1), message: format!("frontmatter: {e}") })?;
    Ok(Lesson { path: path.to_string(), frontmatter, body: body.to_string(), body_line })
}

/// Renders a lesson file: frontmatter in field order, then the body unchanged.
pub fn write(frontmatter: &Frontmatter, body: &str) -> String {
    let value = serde_norway::to_value(frontmatter).expect("frontmatter always serializes");
    format!("---\n{}---\n{body}", crate::yaml::block(&value))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LESSON: &str = "---
schema: 1
id: 7f3a9c2b41
type: pitfall
status: active
when:
  system: tuolumne
  hdf5: \"1.12:1.14.2\"
verified: 2026-09-25
verified_how: ran
tags: [cmake, hdf5]
queries:
  - undefined reference to H5Fopen
  - cmake cannot find hdf5
labels:
  sensitivity: internal
---

# CMake cannot find HDF5 unless HDF5_ROOT is set

## Symptom
Configure fails.
";

    #[test]
    fn parses_valid_lesson() {
        let l = parse("projects/x/a.md", LESSON).unwrap();
        assert_eq!(l.frontmatter.id, "7f3a9c2b41");
        assert_eq!(l.frontmatter.kind, LessonType::Pitfall);
        assert_eq!(l.frontmatter.verified, jiff::civil::date(2026, 9, 25));
        assert_eq!(l.frontmatter.when.len(), 2);
        assert_eq!(l.frontmatter.queries, ["undefined reference to H5Fopen", "cmake cannot find hdf5"]);
        assert!(l.body.starts_with("\n# CMake"));
        assert_eq!(l.body_line, 18);
    }

    #[test]
    fn queries_are_optional_and_written_in_block_style() {
        let text = LESSON.replace("queries:\n  - undefined reference to H5Fopen\n  - cmake cannot find hdf5\n", "");
        let l = parse("a.md", &text).unwrap();
        assert!(l.frontmatter.queries.is_empty() && !write(&l.frontmatter, &l.body).contains("queries"));
        let l = parse("a.md", LESSON).unwrap();
        assert!(write(&l.frontmatter, &l.body).contains("queries:\n  - undefined reference to H5Fopen\n  - cmake cannot find hdf5\n"));
    }

    #[test]
    fn missing_frontmatter() {
        let e = parse("a.md", "# Title\n").unwrap_err();
        assert!(e.message.contains("---"));
    }

    #[test]
    fn unclosed_frontmatter() {
        assert!(parse("a.md", "---\nschema: 1\n").is_err());
    }

    #[test]
    fn unknown_key_is_named() {
        let text = LESSON.replace("verified_how: ran", "verified_how: ran\nverifed: 2026-01-01");
        let e = parse("a.md", &text).unwrap_err();
        assert!(e.message.contains("verifed"), "{}", e.message);
        assert_eq!(e.line, Some(11));
    }

    #[test]
    fn invalid_type_lists_allowed() {
        let e = parse("a.md", &LESSON.replace("type: pitfall", "type: bug")).unwrap_err();
        assert!(e.message.contains("pitfall") && e.message.contains("preference"), "{}", e.message);
    }

    #[test]
    fn round_trip_is_stable() {
        for text in [
            LESSON.to_string(),
            LESSON.replace("7f3a9c2b41", "1234567890"),
            LESSON.replace("7f3a9c2b41", "12e4567890"),
            LESSON.replace("Configure fails.", "trailing  \n\ttab\n```sh\n  x\n```\n"),
        ] {
            let a = parse("a.md", &text).unwrap();
            let once = write(&a.frontmatter, &a.body);
            let b = parse("a.md", &once).unwrap();
            assert_eq!(a.frontmatter, b.frontmatter);
            assert_eq!(write(&b.frontmatter, &b.body), once);
            assert_eq!(b.body, a.body);
        }
    }

    #[test]
    fn write_keeps_field_and_map_order() {
        let text = LESSON.replace("  system: tuolumne\n  hdf5: \"1.12:1.14.2\"", "  zeta: a\n  alpha: b");
        let l = parse("a.md", &text).unwrap();
        let out = write(&l.frontmatter, &l.body);
        let keys: Vec<&str> = out
            .lines()
            .skip(1)
            .take_while(|l| *l != "---")
            .filter(|l| !l.starts_with(' ') && !l.starts_with('-'))
            .map(|l| l.split(':').next().unwrap())
            .collect();
        assert_eq!(keys, ["schema", "id", "type", "status", "when", "verified", "verified_how", "tags", "queries", "labels"]);
        assert!(out.find("zeta").unwrap() < out.find("alpha").unwrap());
    }
}
