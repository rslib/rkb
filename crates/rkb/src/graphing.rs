use std::collections::BTreeMap;

use rkb_core::dupes;
use rkb_core::graph::{self, Edge, Graph};
use rkb_core::triage::Thresholds;
use serde_json::{Value, json};

use crate::list::{cut, marks, width};
use crate::output::{CliError, Output, paint};
use crate::rerankers;
use crate::writes::Env;

fn lesson_json(g: &Graph, i: usize) -> Value {
    let l = &g.lessons[i];
    json!({ "id": l.frontmatter.id, "title": graph::title(l), "path": l.path })
}

fn edge_text(e: &Edge) -> String {
    match e {
        Edge::Similar(s) => format!("similar {s:.2}"),
        Edge::Shares(keys) => format!("shares {}", keys.join(" ")),
        other => other.name().to_string(),
    }
}

fn edge_json(e: &Edge) -> Value {
    match e {
        Edge::Similar(s) => json!({ "kind": e.name(), "similarity": (s * 100.0).round() / 100.0 }),
        Edge::Shares(keys) => json!({ "kind": e.name(), "shared": keys }),
        _ => json!({ "kind": e.name() }),
    }
}

/// The first `limit` related lessons of `i` as JSON rows and human lines.
pub fn related_of(g: &Graph, i: usize, limit: usize, colored: bool) -> (Vec<Value>, String) {
    let m = marks();
    let w = width();
    let mut rows = vec![];
    let mut human = String::new();
    for r in g.related(i).into_iter().take(limit) {
        let l = &g.lessons[r.lesson];
        let edges: Vec<String> = r.edges.iter().map(edge_text).collect();
        let mut row = lesson_json(g, r.lesson);
        row["edges"] = r.edges.iter().map(edge_json).collect();
        rows.push(row);
        human.push_str(&format!(
            "\n  {}\n    {}\n",
            cut(&graph::title(l), w.saturating_sub(4), m),
            paint(colored, "2", &format!("{} {} {} {} {}", l.frontmatter.id, m.sep, l.path, m.sep, edges.join(", ")))
        ));
    }
    (rows, human)
}

fn load(env: &Env) -> Result<Graph, CliError> {
    Ok(Graph::load(&env.root)?)
}

fn find(g: &Graph, id: &str) -> Result<usize, CliError> {
    g.find(id).ok_or_else(|| rkb_core::Error::NotFound(id.to_string()).into())
}

pub fn dupes(env: &Env, min: Option<f64>, check: bool, all: bool) -> Result<Output, CliError> {
    let g = load(env)?;
    let min = min.unwrap_or_else(|| graph::min_similarity(&env.root));
    let mut cache = dupes::load(&env.state);
    let mut why: BTreeMap<(usize, usize), String> = BTreeMap::new();
    let mut note = None;
    if check {
        let todo: Vec<(usize, usize)> = g
            .dupes(min)
            .into_iter()
            .map(|p| (p.0, p.1))
            .filter(|&(a, b)| dupes::get(&cache, &g.lessons[a], &g.lessons[b]).is_none())
            .collect();
        let lessons: Vec<_> = todo.iter().map(|&(a, b)| (&g.lessons[a], &g.lessons[b])).collect();
        note = Some(match rerankers::jev_dupes(&env.root, &lessons) {
            None => "jev is not in the rerank chain, so nothing was checked".to_string(),
            Some(answers) => {
                let mut asked = 0;
                for (&(a, b), r) in todo.iter().zip(answers) {
                    match r {
                        Ok(v) => {
                            asked += 1;
                            cache.insert(dupes::key(&g.lessons[a], &g.lessons[b]), v);
                        }
                        Err(e) => {
                            why.insert((a, b), format!("jev: {e}"));
                        }
                    }
                }
                dupes::save(&env.state, &cache).map_err(|source| rkb_core::Error::Io { path: env.state.join("dupes.json"), source })?;
                let _ = std::fs::remove_file(env.state.join("curate.json"));
                format!("checked {asked} pairs, {} unchecked", why.len())
            }
        });
    }
    let keep = Thresholds::load(&rkb_core::paths::config_dir()).keep;
    let total = g.dupes(min).len();
    let pairs = if all { g.dupes(min) } else { dupes::shown(&g, min, &cache, keep) };
    let hidden = total - pairs.len();
    let verdict = |a: usize, b: usize| dupes::get(&cache, &g.lessons[a], &g.lessons[b]);
    let (c, m, w) = (env.colored, marks(), width());
    let mut human = format!("{} {} {} pairs at or above {min:.2}\n", paint(c, "1", "dupes"), m.sep, pairs.len());
    if let Some(n) = &note {
        human.push_str(&format!("{n}\n"));
    }
    for &(a, b, s) in &pairs {
        for (i, lead) in [(a, format!("{s:.2}")), (b, "    ".to_string())] {
            let l = &g.lessons[i];
            human.push_str(&format!(
                "\n  {} {}\n       {}",
                paint(c, "36", &lead),
                cut(&graph::title(l), w.saturating_sub(9), m),
                paint(c, "2", &format!("{} {} {}", l.frontmatter.id, m.sep, l.path))
            ));
        }
        match (verdict(a, b), why.get(&(a, b))) {
            (Some(v), _) => human.push_str(&format!("\n       {}", paint(c, "33", &format!("{} {:.2}", v.verdict, v.p)))),
            (_, Some(e)) => human.push_str(&format!("\n       {}", paint(c, "33", &format!("unchecked ({e})")))),
            _ => {}
        }
        human.push('\n');
    }
    let help = if pairs.is_empty() {
        human.push_str(&format!("\nNo likely duplicates at {min:.2}. Try `rkb dupes --min <lower>` to see closer pairs.\n"));
        vec![format!("no pair reaches {min:.2}; `--min` tries another threshold")]
    } else {
        vec![
            "Read both with `rkb show <id>` before you decide anything".to_string(),
            "To merge, with the user's agreement: `rkb edit` the lesson to keep, then `rkb supersede <other> --by <kept>`".to_string(),
        ]
    };
    if hidden > 0 {
        human.push_str(&format!("\n{hidden} pairs hidden: Jev rated them different. `rkb dupes --all` shows them.\n"));
    }
    let rows: Vec<Value> = pairs
        .iter()
        .map(|&(a, b, s)| {
            let mut row = json!({ "similarity": (s * 100.0).round() / 100.0, "a": lesson_json(&g, a), "b": lesson_json(&g, b) });
            if let Some(v) = verdict(a, b) {
                row["jev"] = json!({ "verdict": v.verdict, "p": (v.p * 100.0).round() / 100.0 });
            } else if let Some(e) = why.get(&(a, b)) {
                row["jev"] = json!({ "unchecked": e });
            }
            row
        })
        .collect();
    let mut data = json!({ "min_similarity": min, "pairs": rows, "hidden": hidden, "help": help });
    if let Some(n) = note {
        data["check"] = json!(n);
    }
    Ok(Output { data, human, exit: 0, raw: false })
}

pub fn related(env: &Env, id: &str, limit: usize) -> Result<Output, CliError> {
    let g = load(env)?;
    let i = find(&g, id)?;
    let (rows, lines) = related_of(&g, i, limit, env.colored);
    let m = marks();
    let mut human = format!(
        "{} {} {} {} {}\n",
        paint(env.colored, "1", &format!("related {id}")),
        m.sep,
        graph::title(&g.lessons[i]),
        m.sep,
        rows.len()
    );
    human.push_str(&lines);
    if rows.is_empty() {
        human.push_str("\nNo lesson links to it, is superseded with it, or shares words, tags or `when` keys with it.\n");
    }
    Ok(Output { data: json!({ "id": id, "related": rows, "help": ["Run `rkb show <id>` to read a lesson"] }), human, exit: 0, raw: false })
}

#[derive(Clone, Copy)]
pub enum View {
    Counts,
    Clusters,
    Orphans,
    Dot,
    Mermaid,
}

fn quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "'")
}

pub fn graph(env: &Env, view: View) -> Result<Output, CliError> {
    let g = load(env)?;
    let min = graph::min_similarity(&env.root);
    let (c, m, w) = (env.colored, marks(), width());
    let id = |i: usize| g.lessons[i].frontmatter.id.as_str();
    match view {
        View::Dot | View::Mermaid => {
            let dot = matches!(view, View::Dot);
            let mut out = String::from(if dot { "digraph rkb {\n" } else { "graph LR\n" });
            for (i, l) in g.lessons.iter().enumerate() {
                let t = quote(&graph::title(l));
                out.push_str(&if dot { format!("  \"{}\" [label=\"{t}\"];\n", id(i)) } else { format!("  L{}[\"{t}\"]\n", id(i)) });
            }
            for &(a, b) in &g.links {
                out.push_str(&if dot { format!("  \"{}\" -> \"{}\";\n", id(a), id(b)) } else { format!("  L{} --> L{}\n", id(a), id(b)) });
            }
            for &(a, b) in &g.supersedes {
                out.push_str(&if dot {
                    format!("  \"{}\" -> \"{}\" [style=dashed, label=\"superseded by\"];\n", id(a), id(b))
                } else {
                    format!("  L{} -.->|superseded by| L{}\n", id(a), id(b))
                });
            }
            for &(a, b, s) in &g.similar {
                out.push_str(&if dot {
                    format!("  \"{}\" -> \"{}\" [dir=none, style=dotted, label=\"{s:.2}\"];\n", id(a), id(b))
                } else {
                    format!("  L{} ---|{s:.2}| L{}\n", id(a), id(b))
                });
            }
            if dot {
                out.push_str("}\n");
            }
            Ok(Output { data: json!({ "graph": out }), human: out, exit: 0, raw: true })
        }
        View::Clusters => {
            let clusters = g.clusters(min);
            let mut human = format!(
                "{} {} {} groups joined by links, supersedes or similarity at or above {min:.2}\n",
                paint(c, "1", "clusters"),
                m.sep,
                clusters.len()
            );
            for (n, cl) in clusters.iter().enumerate() {
                human.push_str(&format!("\n  {} {} lessons\n", paint(c, "36", &format!("#{}", n + 1)), cl.len()));
                for &i in cl {
                    human.push_str(&format!(
                        "    {} {}\n",
                        paint(c, "2", id(i)),
                        cut(&graph::title(&g.lessons[i]), w.saturating_sub(16), m)
                    ));
                }
            }
            let rows: Vec<Value> = clusters
                .iter()
                .map(|cl| json!({ "size": cl.len(), "lessons": cl.iter().map(|&i| lesson_json(&g, i)).collect::<Vec<_>>() }))
                .collect();
            let help = "A cluster is a candidate to merge into one lesson, or to turn into a skill; ask the user first";
            Ok(Output { data: json!({ "clusters": rows, "help": [help] }), human, exit: 0, raw: false })
        }
        View::Orphans => {
            let orphans = g.orphans();
            let mut human =
                format!("{} {} {} lessons with no link, supersede or similar lesson\n", paint(c, "1", "orphans"), m.sep, orphans.len());
            for &i in &orphans {
                let l = &g.lessons[i];
                human.push_str(&format!(
                    "\n  {}\n    {}\n",
                    cut(&graph::title(l), w.saturating_sub(4), m),
                    paint(c, "2", &format!("{} {} {}", id(i), m.sep, l.path))
                ));
            }
            let rows: Vec<Value> = orphans.iter().map(|&i| lesson_json(&g, i)).collect();
            let help = "An orphan is fine; review it to link it from a related lesson, or archive it if it no longer matters";
            Ok(Output { data: json!({ "orphans": rows, "help": [help] }), human, exit: 0, raw: false })
        }
        View::Counts => {
            let (clusters, orphans) = (g.clusters(min).len(), g.orphans().len());
            let data = json!({
                "lessons": g.lessons.len(),
                "edges": { "link": g.links.len(), "supersedes": g.supersedes.len(), "similar": g.similar.len() },
                "likely_duplicates": g.dupes(min).len(),
                "clusters": clusters,
                "orphans": orphans,
                "help": ["`rkb graph --clusters`, `--orphans`, `--dot` or `--mermaid` for details; `rkb dupes` for likely duplicates"],
            });
            let human = format!(
                "{} lessons  {} links  {} supersedes  {} similar pairs\n{} likely duplicates  {} clusters  {} orphans",
                g.lessons.len(),
                g.links.len(),
                g.supersedes.len(),
                g.similar.len(),
                g.dupes(min).len(),
                clusters,
                orphans
            );
            Ok(Output { data, human, exit: 0, raw: false })
        }
    }
}
