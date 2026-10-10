//! `rkb tier`: the count in each tier and the proven lessons by helpful count. Reads only.

use std::collections::BTreeMap;

use rkb_core::kb::Snapshot;
use rkb_core::tier::{self, Tier};
use rkb_core::usage;
use serde_json::json;

use crate::output::{CliError, Output, paint};
use crate::writes::Env;

/// The title of a lesson, from its H1, or its id when it has none.
fn title(l: &rkb_core::lesson::Lesson) -> String {
    rkb_core::body::scan(&l.body).h1.into_iter().next().map(|(_, t)| t).unwrap_or_else(|| l.frontmatter.id.clone())
}

pub fn tier(env: &Env) -> Result<Output, CliError> {
    let snap = Snapshot::from_dir(&env.root)?;
    let (lessons, _) = snap.lessons();
    let counts = usage::counts(&env.root);
    let mut n: BTreeMap<Tier, usize> = BTreeMap::new();
    let mut proven: Vec<(String, String, f64, f64)> = vec![];
    for l in &lessons {
        let c = counts.get(&l.frontmatter.id);
        let t = tier::of(l.frontmatter.status, c);
        *n.entry(t).or_default() += 1;
        if t == Tier::Proven {
            let c = c.cloned().unwrap_or_default();
            proven.push((l.frontmatter.id.clone(), title(l), c.helpful(), c.harmful()));
        }
    }
    proven.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.3.total_cmp(&b.3)).then(a.0.cmp(&b.0)));
    let count = |t: Tier| n.get(&t).copied().unwrap_or(0);
    let c = env.colored;
    let mut human = format!(
        "{} {}  {} candidate  {} proven  {} archive\n",
        paint(c, "1", "tier"),
        paint(c, "2", "·"),
        count(Tier::Candidate),
        count(Tier::Proven),
        count(Tier::Archive)
    );
    if proven.is_empty() {
        human.push_str("\nno proven lesson yet\n");
    } else {
        human.push_str(&format!("\n{} proven lessons by helpful count:\n", proven.len()));
        for (id, title, helpful, harmful) in &proven {
            human.push_str(&format!("  {id}  helpful {helpful:.1}  harmful {harmful:.1}  {title}\n"));
        }
    }
    let rows: Vec<serde_json::Value> = proven
        .iter()
        .map(|(id, title, helpful, harmful)| json!({ "id": id, "title": title, "helpful": helpful, "harmful": harmful }))
        .collect();
    Ok(Output {
        data: json!({
            "counts": { "candidate": count(Tier::Candidate), "proven": count(Tier::Proven), "archive": count(Tier::Archive) },
            "proven": rows,
        }),
        human,
        exit: 0,
        raw: false,
    })
}
