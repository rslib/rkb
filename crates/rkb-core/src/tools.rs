//! The agent tools every harness offers: their definitions, argument checks, and the lesson that
//! `rkb_add` builds from typed fields. Running them is the CLI's job.

use serde_json::{Map, Value, json};

use crate::lesson::LessonType;

pub const NAMES: [&str; 4] = ["rkb_search", "rkb_show", "rkb_add", "rkb_note"];

const TYPES: [&str; 5] = ["pitfall", "recipe", "fact", "decision", "preference"];

/// The field name of a section heading: `When to use` is `when_to_use`.
pub fn field(heading: &str) -> String {
    heading.to_lowercase().replace(' ', "_")
}

fn kind(name: &str) -> LessonType {
    LessonType::from_name(name).expect("TYPES are lesson types")
}

/// Every section field of every type, with the types that use it.
fn section_fields() -> Vec<(String, Vec<&'static str>)> {
    let mut out: Vec<(String, Vec<&'static str>)> = vec![];
    for t in TYPES {
        for h in kind(t).required_headings() {
            let f = field(h);
            match out.iter_mut().find(|(n, _)| *n == f) {
                Some((_, ts)) => ts.push(t),
                None => out.push((f, vec![t])),
            }
        }
    }
    out
}

/// The four tool definitions: `name`, `description`, and a JSON Schema as `inputSchema`.
pub fn definitions() -> Vec<Value> {
    let mut add_props = json!({
        "type": { "type": "string", "enum": TYPES, "description": "pitfall: an error and its fix; recipe: steps that work; fact: something true here; decision: a choice and why; preference: how the user wants things done" },
        "title": { "type": "string", "description": "What the lesson says, as a short sentence, such as `CMake cannot find HDF5 unless HDF5_ROOT is set`" },
        "topic": { "type": "string", "description": "Topic folder, such as `cmake` or `git`; rkb picks the scope from `when`" },
        "tags": { "type": "array", "items": { "type": "string" }, "description": "A few words to find it by" },
        "when": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Conditions under which it holds, such as {\"hdf5\": \"1.12:1.14.2\"}. Set {\"project\": \"<name>\"} when the claim holds only for one project (its code, decisions or conventions); leave it out for a general rule, and name the project only in evidence" },
        "verified_how": { "type": "string", "enum": ["ran", "read", "told"], "description": "ran: you ran it and saw it work; read: from docs or code; told: the user said so. Default: ran" },
    });
    for (f, types) in section_fields() {
        add_props[&f] = json!({ "type": "string", "description": format!("Section for {}", types.join(", ")) });
    }
    vec![
        json!({
            "name": "rkb_search",
            "description": "Search the user's own knowledge base of lessons learned. Use it BEFORE a web search or a guess whenever you hit an error or a problem that may have been seen before. Returns ranked lessons (TOON) with id, title, whether each applies here, and a summary; read one with rkb_show.",
            "inputSchema": { "type": "object", "properties": {
                "query": { "type": "string", "description": "Words from the problem or the error message" },
                "limit": { "type": "integer", "minimum": 1, "description": "Most results to return. Default: 10" },
                "all": { "type": "boolean", "description": "Also show lessons that do not apply here and other projects' lessons" },
            }, "required": ["query"], "additionalProperties": false },
        }),
        json!({
            "name": "rkb_show",
            "description": "Read one lesson in full by its id (from rkb_search). Check its `applies` result and its Check section before you act on it.",
            "inputSchema": { "type": "object", "properties": {
                "id": { "type": "string", "description": "The lesson id, such as 7f3a9c2b41" },
            }, "required": ["id"], "additionalProperties": false },
        }),
        json!({
            "name": "rkb_add",
            "description": "Record a durable lesson in the user's knowledge base: a fix for an error that can come back, a rule, a decision with its reason, or a recipe that took several tries. Run rkb_search first and do not add a close copy. Fill the sections of the chosen type (pitfall: symptom, cause, fix, evidence; recipe: when_to_use, steps, evidence; fact: statement, evidence; decision: context, decision, why, rejected_options; preference: rule, why, how_to_apply). Keep the real error text and the command that fixed it; never invent a fix. rkb checks, files and commits it, and may return needs_user with a question for the user.",
            "inputSchema": { "type": "object", "properties": add_props, "required": ["type", "title", "topic"], "additionalProperties": false },
        }),
        json!({
            "name": "rkb_note",
            "description": "Save a short finding to the rkb inbox when you cannot write the full lesson now, with the error text and what fixed it. Inbox items become lessons later with the distill command.",
            "inputSchema": { "type": "object", "properties": {
                "text": { "type": "string", "description": "What you found" },
            }, "required": ["text"], "additionalProperties": false },
        }),
    ]
}

/// Checks `args` against the tool's schema: required fields, no unknown fields, types and enums.
/// The error names the field.
pub fn check(name: &str, args: &Value) -> Result<(), String> {
    let def = definitions()
        .into_iter()
        .find(|d| d["name"] == name)
        .ok_or_else(|| format!("unknown tool `{name}`; tools: {}", NAMES.join(", ")))?;
    let schema = &def["inputSchema"];
    let empty = Map::new();
    let obj = match args {
        Value::Object(o) => o,
        Value::Null => &empty,
        _ => return Err("the arguments must be a JSON object".into()),
    };
    for r in schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if obj.get(r).is_none_or(Value::is_null) {
            return Err(format!("`{r}` is required"));
        }
    }
    for (k, v) in obj {
        let Some(p) = schema["properties"].get(k) else {
            return Err(format!("`{k}` is not an argument of {name}"));
        };
        let ok = match p["type"].as_str() {
            Some("string") => v.as_str().is_some_and(|s| p["enum"].as_array().is_none_or(|e| e.iter().any(|x| x == s))),
            Some("integer") => v.as_u64().is_some_and(|n| n >= 1),
            Some("boolean") => v.is_boolean(),
            Some("array") => v.as_array().is_some_and(|a| a.iter().all(Value::is_string)),
            Some("object") => v.as_object().is_some_and(|o| o.values().all(Value::is_string)),
            _ => true,
        };
        if !ok {
            let want = match p["enum"].as_array() {
                Some(e) => format!("one of {}", e.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")),
                None => format!("a {}", p["type"].as_str().unwrap_or("value")),
            };
            return Err(format!("`{k}` must be {want}"));
        }
    }
    Ok(())
}

/// The topic and the lesson Markdown for checked `rkb_add` arguments.
pub fn lesson(args: &Value) -> Result<(String, String), String> {
    check("rkb_add", args)?;
    let type_name = args["type"].as_str().unwrap_or_default();
    let t = kind(type_name);
    let text = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let title = text("title").ok_or("`title` is empty")?;
    let topic = text("topic").ok_or("`topic` is empty")?;
    let mut fm = serde_norway::Mapping::new();
    fm.insert("type".into(), type_name.into());
    fm.insert("verified_how".into(), text("verified_how").unwrap_or("ran").into());
    if let Some(tags) = args["tags"].as_array().filter(|a| !a.is_empty()) {
        fm.insert("tags".into(), tags.iter().filter_map(Value::as_str).map(serde_norway::Value::from).collect::<Vec<_>>().into());
    }
    if let Some(when) = args["when"].as_object().filter(|w| !w.is_empty()) {
        let mut m = serde_norway::Mapping::new();
        for (k, v) in when {
            m.insert(k.as_str().into(), v.as_str().unwrap_or_default().into());
        }
        fm.insert("when".into(), m.into());
    }
    let mut body = format!("# {title}\n");
    for h in t.required_headings() {
        let f = field(h);
        let s = text(&f).ok_or_else(|| format!("a {type_name} needs `{f}`"))?;
        body.push_str(&format!("\n## {h}\n{s}\n"));
    }
    let fm = serde_norway::to_string(&fm).map_err(|e| e.to_string())?;
    Ok((topic.to_string(), format!("---\n{fm}---\n\n{body}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_and_checks() {
        let defs = definitions();
        assert_eq!(defs.iter().map(|d| d["name"].as_str().unwrap()).collect::<Vec<_>>(), NAMES);
        assert!(defs[0]["description"].as_str().unwrap().contains("BEFORE a web search"));
        assert!(defs[2]["inputSchema"]["properties"]["when_to_use"].is_object());
        assert_eq!(check("rkb_show", &json!({})).unwrap_err(), "`id` is required");
        assert_eq!(check("rkb_search", &json!({ "query": "x", "limt": 3 })).unwrap_err(), "`limt` is not an argument of rkb_search");
        assert!(check("rkb_add", &json!({ "type": "bug", "title": "t", "topic": "x" })).unwrap_err().contains("one of pitfall"));
        assert!(check("rkb_nope", &json!({})).is_err());
        assert!(check("rkb_search", &json!({ "query": "x", "limit": 5, "all": true })).is_ok());
    }

    #[test]
    fn lesson_from_fields() {
        let args = json!({
            "type": "pitfall", "title": "CMake keeps a failed find_package", "topic": "cmake",
            "symptom": "Still not found.", "cause": "The cache.", "fix": "cmake --fresh", "evidence": "It worked.",
            "tags": ["cmake"], "when": { "cmake": "3.24:" },
        });
        let (topic, text) = lesson(&args).unwrap();
        assert_eq!(topic, "cmake");
        let fm: serde_norway::Value = serde_norway::from_str(text.split("---\n").nth(1).unwrap()).unwrap();
        assert_eq!(
            (fm["type"].as_str(), fm["tags"][0].as_str(), fm["when"]["cmake"].as_str()),
            (Some("pitfall"), Some("cmake"), Some("3.24:"))
        );
        assert!(text.contains("\n# CMake keeps a failed find_package\n"), "{text}");
        let heads: Vec<&str> = text.lines().filter_map(|l| l.strip_prefix("## ")).collect();
        assert_eq!(heads, ["Symptom", "Cause", "Fix", "Evidence"]);

        let recipe = json!({ "type": "recipe", "title": "t", "topic": "git", "when_to_use": "w", "evidence": "e" });
        assert_eq!(lesson(&recipe).unwrap_err(), "a recipe needs `steps`");
    }
}
