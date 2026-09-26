mod hook;
mod list;
mod output;
mod searching;
mod session;
mod setup;
mod writes;

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use output::{CliError, Format, Output, color, cut, paint};
use rkb_core::conditions::Verdict;
use rkb_core::lint::{LintEnv, Severity};
use rkb_core::{init, kb, lint, paths, usage};
use serde_json::{Value, json};

const MESSAGE_LIMIT: usize = 200;

/// Read and grow a knowledge base of lessons learned.
#[derive(Parser)]
#[command(name = "rkb", version, after_help = "Example:\n  rkb lint")]
struct Cli {
    /// Output format. Default: RKB_FORMAT, then human on a terminal and toon otherwise.
    #[arg(long, global = true, value_enum)]
    format: Option<Format>,
    /// Use this project instead of matching one (also RKB_PROJECT).
    #[arg(long, global = true)]
    project: Option<String>,
    /// Use this system instead of matching one (also RKB_SYSTEM).
    #[arg(long, global = true)]
    system: Option<String>,
    /// Set or override a fact for `when` conditions, such as --with hdf5=1.14.3. Repeatable.
    #[arg(long = "with", global = true, value_name = "KEY=VALUE")]
    with: Vec<String>,
    /// With no command, rkb shows where it runs and what needs attention.
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new knowledge base at $RKB_HOME (default ~/Personal/kb).
    #[command(after_help = "Example:\n  RKB_HOME=~/Personal/kb rkb init\n  rkb init --clone git@github.com:you/kb.git")]
    Init {
        /// Set up an existing knowledge base from this git URL or path, for a second machine.
        #[arg(long, value_name = "URL")]
        clone: Option<String>,
    },
    /// Check every lesson, image and config file.
    #[command(after_help = "Example:\n  rkb lint\n  rkb lint --fix\n  rkb lint --staged")]
    Lint {
        /// Check the git index instead of the working tree, as the pre-commit hook does.
        #[arg(long)]
        staged: bool,
        /// Do not cut long messages.
        #[arg(long)]
        full: bool,
        /// Rewrite flow-style frontmatter (`[a, b]`, `{k: v}`) in block style first. Does not commit.
        #[arg(long, conflicts_with = "staged")]
        fix: bool,
    },
    /// Print one lesson in full, with its content hash and use records.
    #[command(after_help = "Example:\n  rkb show 7f3a9c2b41")]
    Show {
        /// The lesson id, the `id:` field of its frontmatter.
        id: String,
    },
    /// Find lessons that answer a question or an error, ranked, only those that can hold here.
    #[command(after_help = "Example:\n  rkb search \"cmake cannot find hdf5\"\n  rkb search --literal \"Could NOT find HDF5\"")]
    Search {
        /// Words from the problem or the error.
        #[arg(required_unless_present_any = ["literal", "regex"], allow_hyphen_values = true)]
        query: Option<String>,
        /// Find lessons that contain this exact text.
        #[arg(long, conflicts_with_all = ["regex", "query"], allow_hyphen_values = true)]
        literal: Option<String>,
        /// Find lessons whose text matches this regular expression.
        #[arg(long, conflicts_with = "query", allow_hyphen_values = true)]
        regex: Option<String>,
        /// Search every folder and show lessons that do not apply here.
        #[arg(long)]
        all: bool,
        /// `all` also shows superseded and archived lessons.
        #[arg(long, value_parser = ["all"])]
        status: Option<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Look up a lesson by a rough title, slug, path, tag or id.
    #[command(after_help = "Example:\n  rkb find hdf5 root")]
    Find {
        #[arg(required = true, num_args = 1..)]
        words: Vec<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Measure search on a query file: recall at 5 and mean reciprocal rank.
    #[command(after_help = "Example:\n  rkb eval\n  rkb eval --queries tests/fixtures/search/queries.toml --min-recall 0.9")]
    Eval {
        /// Default: $RKB_HOME/eval/queries.toml.
        #[arg(long)]
        queries: Option<String>,
        /// Exit non-zero when recall at 5 is below this.
        #[arg(long)]
        min_recall: Option<f64>,
    },
    /// Put the rkb skill, hooks and confirm gate into Claude Code, pi and omp.
    #[command(after_help = "Example:\n  rkb install\n  rkb install --list\n  rkb install claude --uninstall")]
    Install {
        /// claude, pi or omp. Default: every detected harness.
        harnesses: Vec<String>,
        /// Show what is installed and whether it matches this rkb.
        #[arg(long, conflicts_with = "uninstall")]
        list: bool,
        /// Remove what `rkb install` added.
        #[arg(long)]
        uninstall: bool,
    },
    /// Print the current project, system and lesson counts in at most 5 lines, for a session hook.
    #[command(after_help = "Example:\n  rkb context")]
    Context,
    /// Check git, the hook, PATH, lint, locks, requests and matching, with a fix for each problem.
    #[command(after_help = "Example:\n  rkb doctor\n  rkb doctor --break-lock")]
    Doctor {
        /// Remove a stuck write lock, after the user confirms.
        #[arg(long)]
        break_lock: bool,
    },
    /// Show what the knowledge base holds: all topics, one scope, or the lessons of one topic.
    #[command(after_help = "Example:\n  rkb list\n  rkb list general/cpp")]
    List {
        /// A scope folder (general, projects/dftracer) or a topic folder (general/cpp).
        folder: Option<String>,
    },
    /// Add a lesson read from stdin. rkb sets id, schema, status and the path.
    #[command(after_help = "Example:\n  rkb add --topic cmake < lesson.md\n  rkb add --type pitfall --template")]
    Add {
        /// Topic folder, such as cmake. An alias from a topic note works too.
        #[arg(long)]
        topic: Option<String>,
        /// Lesson type, for --template or for writing the lesson in $EDITOR.
        #[arg(long = "type", value_parser = writes::TYPES)]
        kind: Option<String>,
        /// Print a skeleton for --type and write nothing.
        #[arg(long)]
        template: bool,
    },
    /// Replace a lesson with the file read from stdin, or edit it in $EDITOR.
    #[command(after_help = "Example:\n  rkb edit 7f3a9c2b41 --base 3f9a1c0d2e4b < lesson.md")]
    Edit {
        id: String,
        /// The hash `rkb show` printed; the edit fails if the lesson changed since.
        #[arg(long)]
        base: Option<String>,
    },
    /// Mark a lesson stale because it looks wrong.
    #[command(after_help = "Example:\n  rkb flag 7f3a9c2b41 --reason \"fix failed with HDF5 1.14.3\"")]
    Flag {
        id: String,
        #[arg(long)]
        reason: String,
    },
    /// Mark a wrong lesson as replaced by another. Asks the user, because search then hides it.
    #[command(after_help = "Example:\n  rkb supersede 7f3a9c2b41 --by 0a1b2c3d4e --reason \"the flag is wrong on 1.14\"")]
    Supersede {
        id: String,
        /// The lesson that replaces it.
        #[arg(long)]
        by: String,
        #[arg(long)]
        reason: String,
    },
    /// Hide a lesson, or every lesson under a folder, that is still true but no longer relevant. Asks the user.
    #[command(
        after_help = "Example:\n  rkb archive 7f3a9c2b41 --reason \"quartz retired 2027-01\"\n  rkb archive systems/quartz --reason \"quartz retired 2027-01\""
    )]
    Archive {
        /// A lesson id or a folder such as systems/quartz.
        target: String,
        #[arg(long)]
        reason: String,
    },
    /// Bring an archived lesson back into search.
    #[command(after_help = "Example:\n  rkb unarchive 7f3a9c2b41")]
    Unarchive { id: String },
    /// Record that you applied a lesson and whether it worked.
    #[command(after_help = "Example:\n  rkb used 7f3a9c2b41 --worked\n  rkb used 7f3a9c2b41 --failed --reason \"still fails on 1.14.3\"")]
    Used {
        id: String,
        #[arg(long, conflicts_with = "failed", required_unless_present = "failed")]
        worked: bool,
        /// Also flags the lesson with --reason.
        #[arg(long, requires = "reason")]
        failed: bool,
        #[arg(long)]
        reason: Option<String>,
    },
    /// List the commits that changed a lesson, newest first.
    #[command(after_help = "Example:\n  rkb log 7f3a9c2b41")]
    Log { id: String },
    /// Run a decision the user made on a needs_user request.
    #[command(after_help = "Example:\n  rkb confirm r-4f2a9c --choice \"create general/cmake\"")]
    Confirm {
        request: String,
        /// One of the request's options, exactly as printed.
        #[arg(long)]
        choice: String,
    },
    /// Check a folder of lesson files and add them all after one confirmation.
    #[command(
        after_help = "Layout: <dir>/<topic>/<name>.md, each file as `rkb add` reads it.\n\nExample:\n  rkb import /tmp/notes-to-import"
    )]
    Import {
        /// The folder with the lesson files.
        dir: std::path::PathBuf,
    },
    /// Pull lessons from a git remote, check them, and push when every check passes.
    #[command(after_help = "Example:\n  rkb sync\n  rkb sync --remote cluster\n  rkb sync --bundle /mnt/usb/kb.bundle")]
    Sync {
        /// The git remote. Default: the branch's upstream remote, else origin.
        #[arg(long, conflicts_with = "bundle")]
        remote: Option<String>,
        /// Sync through a git bundle file that both machines read and write, for a machine with no ssh path.
        #[arg(long, value_name = "FILE")]
        bundle: Option<std::path::PathBuf>,
    },
    /// Run by Claude Code hooks: reads the event's JSON on stdin and prints the reply. Never fails.
    #[command(after_help = "Example:\n  echo '{\"session_id\":\"s1\",\"cwd\":\".\"}' | rkb hook session-start")]
    Hook {
        /// session-start, pre-tool, tool-ok, tool-failed, prompt or stop.
        event: String,
        /// The harness that calls: claude-code, pi or omp. Picks the heartbeat file only.
        #[arg(long, default_value = "claude-code")]
        harness: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(Cmd::Hook { event, harness }) = &cli.cmd {
        hook::run(event, harness);
        return ExitCode::SUCCESS;
    }
    let format = match output::resolve(cli.format) {
        Ok(f) => f,
        Err(e) => {
            output::print_error(Format::Human, &e);
            return ExitCode::from(1);
        }
    };
    let hints = rkb_core::matching::Hints { project: cli.project, system: cli.system };
    let mut with = vec![];
    for w in &cli.with {
        match w.split_once('=') {
            Some((k, v)) if !k.is_empty() => with.push((k.to_string(), v.to_string())),
            _ => {
                let e = CliError::new(
                    output::ErrorCode::Usage,
                    format!("--with {w} has no `=`"),
                    "write --with key=value, such as --with hdf5=1.14.3",
                );
                output::print_error(format, &e);
                return ExitCode::from(2);
            }
        }
    }
    match run(cli.cmd, format, &hints, &with) {
        Ok(out) => {
            output::print(format, &out);
            ExitCode::from(out.exit)
        }
        Err(e) => {
            output::print_error(format, &e);
            ExitCode::from(if e.code == output::ErrorCode::Usage { 2 } else { 1 })
        }
    }
}

fn run(cmd: Option<Cmd>, format: Format, hints: &rkb_core::matching::Hints, with: &[(String, String)]) -> Result<Output, CliError> {
    let root = kb::home();
    let colored = format == Format::Human && color();
    let state = paths::state_dir();
    let needs_place =
        matches!(cmd, None | Some(Cmd::Context | Cmd::Doctor { .. } | Cmd::Add { .. } | Cmd::Show { .. } | Cmd::Search { .. }));
    let place = if needs_place && kb::open(&root).is_ok() {
        let cwd = std::env::current_dir().unwrap_or_else(|_| root.clone());
        Some(rkb_core::state::locate(&root, &cwd, hints, &state)?)
    } else {
        None
    };
    let env = writes::Env { root: root.clone(), lint: LintEnv::from_process(), state, colored, place };
    let Some(cmd) = cmd else {
        kb::open(&root)?;
        let s = rkb_core::state::build(&root, env.place.clone().unwrap_or_default(), &env.state)?;
        return Ok(session::status(&env, &s));
    };
    match cmd {
        Cmd::Search { query, literal, regex, all, status, limit } => {
            kb::open(&root)?;
            let mode = match (query, literal, regex) {
                (_, Some(t), _) => rkb_core::search::Mode::Literal(t),
                (_, _, Some(p)) => rkb_core::search::Mode::Regex(p),
                (Some(q), _, _) => rkb_core::search::Mode::Ranked(q),
                _ => unreachable!("clap requires one"),
            };
            let facts = rkb_core::conditions::Facts::gather(&root, &env.place.clone().unwrap_or_default(), with);
            let opts = rkb_core::search::Options { all, every_status: status.is_some(), limit };
            searching::search(&env, &facts, mode, opts)
        }
        Cmd::Find { words, limit } => {
            kb::open(&root)?;
            searching::find(&env, &words.join(" "), limit)
        }
        Cmd::Eval { queries, min_recall } => {
            kb::open(&root)?;
            searching::eval(&env, queries, min_recall)
        }
        Cmd::Install { harnesses, list, uninstall } => setup::install(&env, harnesses, list, uninstall),
        Cmd::Context => {
            kb::open(&root)?;
            let s = rkb_core::state::build(&root, env.place.clone().unwrap_or_default(), &env.state)?;
            Ok(session::context(&s))
        }
        Cmd::Doctor { break_lock } => session::doctor(&env, &env.place.clone().unwrap_or_default(), break_lock),
        Cmd::List { folder } => {
            kb::open(&root)?;
            let listing = rkb_core::list::list(&root, folder.as_deref())?;
            Ok(list::render(&listing, &root, colored))
        }
        Cmd::Add { topic, kind, template } => writes::add(&env, topic, kind, template),
        Cmd::Edit { id, base } => writes::edit(&env, id, base),
        Cmd::Flag { id, reason } => writes::flag(&env, id, reason),
        Cmd::Supersede { id, by, reason } => writes::lifecycle(&env, rkb_core::request::Action::Supersede { id, by, reason }),
        Cmd::Archive { target, reason } => writes::lifecycle(&env, rkb_core::request::Action::Archive { target, reason }),
        Cmd::Unarchive { id } => writes::lifecycle(&env, rkb_core::request::Action::Unarchive { id }),
        Cmd::Used { id, failed, reason, .. } => writes::used(&env, id, failed, reason),
        Cmd::Log { id } => writes::log(&env, id),
        Cmd::Confirm { request, choice } => writes::confirm(&env, request, choice),
        Cmd::Hook { .. } => unreachable!("main runs hooks first"),
        Cmd::Import { dir } => writes::import(&env, &dir),
        Cmd::Sync { remote, bundle } => {
            kb::open(&root)?;
            let other = match &bundle {
                Some(file) => rkb_core::sync::Other::Bundle(file),
                None => rkb_core::sync::Other::Remote(remote.as_deref()),
            };
            let s = rkb_core::sync::sync(&root, other, &env.lint)?;
            Ok(sync_output(&s, colored))
        }
        Cmd::Init { clone: Some(url) } => {
            init::clone(&root, &url)?;
            let path = root.display().to_string();
            Ok(Output {
                human: format!("Cloned {url} into {path}, with the pre-commit hook.\nNext: run `rkb install` on this machine, then `rkb`."),
                data: json!({ "cloned": url, "path": path, "help": ["Run `rkb install` on this machine, then `rkb` to see where you are"] }),
                exit: 0,
                raw: false,
            })
        }
        Cmd::Init { clone: None } => {
            init::init(&root)?;
            let path = root.display().to_string();
            Ok(Output {
                human: format!("Created knowledge base at {path}\nNext: add lessons as general/<topic>/<file>.md, then run `rkb lint`."),
                data: json!({
                    "created": path,
                    "help": ["Add lessons as general/<topic>/<file>.md, projects/<project>/<topic>/<file>.md or systems/<system>/<topic>/<file>.md, then run `rkb lint`"],
                }),
                exit: 0,
                raw: false,
            })
        }
        Cmd::Lint { staged, full, fix } => {
            kb::open(&root)?;
            let env = LintEnv::from_process();
            let fixed = if fix { lint::fix_flow_style(&root)? } else { vec![] };
            let findings = if staged { lint::lint_staged(&root, &env)? } else { lint::lint_tree(&root, &env)? };
            let mut out = lint_output(&findings, full, colored);
            if fix {
                out.data["fixed"] = json!(fixed);
                let list: String = fixed.iter().map(|p| format!("fixed   {p}\n")).collect();
                out.human = format!("{list}{}", out.human);
            }
            Ok(out)
        }
        Cmd::Show { id } => {
            kb::open(&root)?;
            let (lesson, text) = kb::find(&root, &id)?;
            let fm = &lesson.frontmatter;
            let title = rkb_core::body::scan(&lesson.body).h1.into_iter().next().map(|(_, t)| t).unwrap_or_default();
            let days = jiff::Zoned::now().date().since(fm.verified).map(|s| s.get_days()).unwrap_or(0);
            let age = match days {
                0 => "today".to_string(),
                1 => "1 day ago".to_string(),
                n if n < 0 => "in the future".to_string(),
                n => format!("{n} days ago"),
            };
            let hash = rkb_core::write::content_hash(text.as_bytes());
            let (worked, failed) = usage::last(&env.state, &fm.id);
            let uses =
                format!("hash {hash}  worked {}  failed {}", worked.as_deref().unwrap_or("never"), failed.as_deref().unwrap_or("never"));
            let place = env.place.clone().unwrap_or_default();
            let facts = rkb_core::conditions::Facts::gather(&root, &place, with);
            let applies = rkb_core::conditions::evaluate(&fm.when, &facts);
            let applies_color = match applies.result {
                Verdict::Yes => "32",
                Verdict::No => "31",
                Verdict::Unknown => "33",
            };
            let mut applies_text = paint(colored, applies_color, &format!("applies here: {}", applies.summary()));
            if applies.result != Verdict::Yes {
                for k in &applies.keys {
                    applies_text.push_str(&format!("\n  {:<8} {:<10} {}", k.result.as_str(), k.key, k.reason));
                }
            }
            let human = format!(
                "{}\n{}  {}  {}\n{}\n{}\n\n{}",
                paint(colored, "1", &title),
                paint(colored, "2", &lesson.path),
                paint(colored, "2", &format!("{:?}", fm.status).to_lowercase()),
                paint(colored, "2", &format!("verified {} ({age}, {:?})", fm.verified, fm.verified_how).to_lowercase()),
                paint(colored, "2", &uses),
                applies_text,
                text
            );
            let data = json!({
                "id": fm.id,
                "path": lesson.path,
                "title": title,
                "hash": hash,
                "last_worked": worked,
                "last_failed": failed,
                "applies": {
                    "result": applies.result,
                    "keys": applies.keys.iter().map(|k| json!({
                        "key": k.key,
                        "want": k.want,
                        "have": k.have.clone().unwrap_or_default(),
                        "result": k.result,
                        "reason": k.reason,
                    })).collect::<Vec<_>>(),
                },
                "frontmatter": serde_json::to_value(fm).unwrap_or(Value::Null),
                "body": lesson.body,
            });
            Ok(Output { data, human, exit: 0, raw: false })
        }
    }
}

fn commit_rows(label: &str, commits: &[rkb_core::sync::Commit], colored: bool) -> String {
    if commits.is_empty() {
        return String::new();
    }
    let mut out = format!("{label} {}\n", commits.len());
    for c in commits {
        out.push_str(&format!("  {}  {}\n", paint(colored, "2", &c.hash), c.subject));
    }
    out
}

fn sync_output(s: &rkb_core::sync::Synced, colored: bool) -> Output {
    let head = format!("{}\n", paint(colored, "2", &format!("remote {}, branch {}", s.remote, s.branch)));
    let pulled = commit_rows("pulled", &s.pulled, colored);
    if s.blocked() {
        let lint = lint_output(&s.findings, false, colored);
        let mut data = json!({
            "status": "not_pushed",
            "remote": s.remote,
            "branch": s.branch,
            "pulled": s.pulled,
            "errors": lint.data["errors"],
            "warnings": lint.data["warnings"],
            "findings": lint.data["findings"],
        });
        data["help"] = json!(["Fix each error with `rkb edit`, then run `rkb sync` again; the pulled commits are kept"]);
        let human = format!(
            "{head}{pulled}{}\n{}",
            lint.human,
            paint(colored, "31", "not pushed: fix the errors with `rkb edit`, then run `rkb sync` again")
        );
        return Output { data, human, exit: 1, raw: false };
    }
    let pushed = commit_rows("pushed", &s.pushed, colored);
    let mut human = format!("{head}{pulled}{pushed}");
    if s.pulled.is_empty() && s.pushed.is_empty() {
        human.push_str("up to date\n");
    }
    let warnings = s.findings.len();
    if warnings > 0 {
        human.push_str(&format!("{}\n", paint(colored, "33", &format!("{warnings} lint warnings; run `rkb lint` to see them"))));
    }
    let data = json!({
        "status": "done",
        "remote": s.remote,
        "branch": s.branch,
        "pulled": s.pulled,
        "pushed": s.pushed,
        "warnings": warnings,
        "help": ["Run `rkb` to see where you are"],
    });
    Output { data, human, exit: 0, raw: false }
}

fn lint_output(findings: &[lint::Finding], full: bool, colored: bool) -> Output {
    let errors = findings.iter().filter(|f| f.severity == Severity::Error).count();
    let warnings = findings.len() - errors;
    let message = |m: &str| if full { m.to_string() } else { cut(m, MESSAGE_LIMIT, "rkb lint --full") };

    let rows: Vec<Value> = findings
        .iter()
        .map(|f| {
            json!({
                "path": f.path,
                "line": f.line,
                "severity": f.severity,
                "rule": f.rule,
                "message": message(&f.message),
            })
        })
        .collect();
    let mut data = json!({ "errors": errors, "warnings": warnings, "findings": rows });
    if errors > 0 {
        data["help"] = json!(["Fix each error, then run `rkb lint` again"]);
    }

    let locs: Vec<String> = findings.iter().map(|f| f.line.map_or_else(|| f.path.clone(), |l| format!("{}:{l}", f.path))).collect();
    let width = locs.iter().map(String::len).max().unwrap_or(0);
    let rule_width = findings.iter().map(|f| f.rule.len()).max().unwrap_or(0);
    let mut human = String::new();
    for (f, loc) in findings.iter().zip(&locs) {
        let sev = match f.severity {
            Severity::Error => paint(colored, "31", "error  "),
            Severity::Warning => paint(colored, "33", "warning"),
        };
        human.push_str(&format!("{loc:width$}  {sev}  {:rule_width$}  {}\n", f.rule, message(&f.message)));
    }
    human.push_str(&format!("{errors} errors, {warnings} warnings"));
    Output { data, human, exit: u8::from(errors > 0), raw: false }
}
