mod agent;
mod graphing;
mod hook;
mod inbox;
mod list;
mod mcp;
mod output;
mod rerankers;
mod searching;
mod session;
mod setup;
mod site;
mod writes;

use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
use output::{CliError, Format, Output, color, cut, paint};
use rkb_core::conditions::Verdict;
use rkb_core::lint::{LintEnv, Severity};
use rkb_core::{init, kb, lint, paths, usage};
use serde_json::{Value, json};

const MESSAGE_LIMIT: usize = 200;

/// What rkb is, in one sentence: the top-level help and the no-command view share it.
pub const ABOUT: &str = "Read and grow a knowledge base of lessons learned.";

#[derive(Parser)]
#[command(name = "rkb", version, about = ABOUT, after_help = "Example:\n  rkb lint")]
struct Cli {
    /// Output format. Default: RKB_FORMAT, then human.
    #[arg(long, global = true, value_enum)]
    format: Option<Format>,
    /// TOON output for agents; the same as --format toon.
    #[arg(long, global = true)]
    toon: bool,
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
        /// Rerank with this backend for this call instead of the chain in kb.toml: laya or bm25.
        #[arg(long, conflicts_with = "no_model")]
        rerank: Option<String>,
        /// Rank with BM25 only; the same as --rerank bm25.
        #[arg(long)]
        no_model: bool,
    },
    /// Look up a lesson by a rough title, slug, path, tag or id.
    #[command(after_help = "Example:\n  rkb find hdf5 root")]
    Find {
        #[arg(required = true, num_args = 1..)]
        words: Vec<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// List pairs of lessons that may say the same thing, most similar first. Changes nothing.
    #[command(after_help = "Example:\n  rkb dupes\n  rkb dupes --min 0.3")]
    Dupes {
        /// Similarity from 0 to 1. Default: `dupes.min_similarity` in kb.toml, or 0.4.
        #[arg(long)]
        min: Option<f64>,
    },
    /// List the lessons connected to one lesson: links, supersedes, similar words, shared tags.
    #[command(after_help = "Example:\n  rkb related 7f3a9c2b41")]
    Related {
        id: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Show how lessons connect: counts, clusters, orphans, or the whole graph for drawing.
    #[command(after_help = "Example:\n  rkb graph\n  rkb graph --clusters\n  rkb graph --mermaid > kb.mmd")]
    #[command(group(clap::ArgGroup::new("view").args(["clusters", "orphans", "dot", "mermaid"])))]
    Graph {
        /// Groups of close lessons, candidates to merge.
        #[arg(long)]
        clusters: bool,
        /// Lessons with no link, supersede or similar lesson.
        #[arg(long)]
        orphans: bool,
        /// The graph in Graphviz DOT.
        #[arg(long)]
        dot: bool,
        /// The graph as a Mermaid flowchart.
        #[arg(long)]
        mermaid: bool,
    },
    /// Measure search on a query file: recall at 5 and mean reciprocal rank.
    #[command(
        after_help = "Example:\n  rkb eval\n  rkb eval --queries tests/fixtures/search/queries.toml --min-recall 0.9\n  rkb eval --rerank laya --recall-sweep"
    )]
    Eval {
        /// Default: $RKB_HOME/eval/queries.toml.
        #[arg(long)]
        queries: Option<String>,
        /// Exit non-zero when recall at 5 is below this.
        #[arg(long)]
        min_recall: Option<f64>,
        /// Rerank with this backend; the eval fails if it cannot run, so a report never mixes backends.
        #[arg(long, default_value = "bm25")]
        rerank: String,
        /// With a model, also count what recall would add over a grid of thresholds and margins.
        #[arg(long)]
        recall_sweep: bool,
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
        /// Lesson text from the rkb_add tool instead of stdin.
        #[arg(skip)]
        text: Option<String>,
        /// An image (png, jpg, webp or svg) to store with the lesson, metadata removed. Repeatable. Link it by file name.
        #[arg(long = "asset", value_name = "FILE")]
        assets: Vec<String>,
    },
    /// Replace a lesson with the file read from stdin, or edit it in $EDITOR.
    #[command(after_help = "Example:\n  rkb edit 7f3a9c2b41 --base 3f9a1c0d2e4b < lesson.md")]
    Edit {
        id: String,
        /// The hash `rkb show` printed; the edit fails if the lesson changed since.
        #[arg(long)]
        base: Option<String>,
        /// Lesson text from the rkb_edit tool instead of stdin.
        #[arg(skip)]
        text: Option<String>,
        /// An image (png, jpg, webp or svg) to store with the lesson, metadata removed; replaces one of the same name. Repeatable.
        #[arg(long = "asset", value_name = "FILE")]
        assets: Vec<String>,
    },
    /// Mark a lesson stale because it looks wrong.
    #[command(after_help = "Example:\n  rkb flag 7f3a9c2b41 --reason \"fix failed with HDF5 1.14.3\"")]
    Flag {
        id: String,
        #[arg(long)]
        reason: String,
    },
    /// List lessons that may no longer matter (retired place, unsupported versions, long stale, missing commit). Changes nothing.
    #[command(after_help = "Example:\n  rkb review\n  rkb review systems/quartz")]
    Review {
        /// Only lessons under this folder.
        folder: Option<String>,
        /// Instead, summarize this machine's sessions: signals, injections and whether they helped.
        #[arg(long)]
        signals: bool,
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
    /// Move a lesson (and its assets) to another topic folder and rewrite every link to it.
    #[command(after_help = "Example:\n  rkb move 7f3a9c2b41 general/build")]
    Move {
        id: String,
        /// The topic folder, such as general/build or projects/dftracer/cmake.
        folder: String,
    },
    /// Rename a lesson file (and its assets folder) in place and rewrite every link to it.
    #[command(after_help = "Example:\n  rkb rename 7f3a9c2b41 cmake-needs-hdf5-root")]
    Rename {
        id: String,
        /// The new file name without `.md`: lowercase letters, digits and `-`.
        slug: String,
    },
    /// Approve a lesson's Check or Probe script to run on this machine. Asks the user and shows the whole script.
    #[command(after_help = "Example:\n  rkb approve 7f3a9c2b41 check\n  rkb approve facts.hdf5")]
    Approve {
        /// A lesson id, or `facts.<key>` for a fact command in kb.toml.
        id: String,
        /// For a lesson: check or probe.
        #[arg(value_parser = ["check", "probe"])]
        kind: Option<String>,
    },
    /// Run a lesson's approved Check and update its status: a fail makes it stale, a pass makes it checked.
    #[command(after_help = "Example:\n  rkb verify 7f3a9c2b41\n  rkb verify --auto")]
    Verify {
        #[arg(required_unless_present = "auto")]
        id: Option<String>,
        /// Run every approved Check that can apply here. Never asks and never makes a stale lesson active; safe for cron.
        #[arg(long, conflicts_with = "id")]
        auto: bool,
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
        /// The session the lesson was used in. Default: the harness's session, else the session that injected it.
        #[arg(long)]
        session: Option<String>,
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
    /// Serve the agent tools over MCP on stdin and stdout; Claude Code starts this through the rkb plugin.
    #[command(after_help = "Example:\n  rkb mcp")]
    Mcp,
    /// Print the agent tools every harness offers: names, descriptions and argument schemas.
    #[command(after_help = "Example:\n  rkb tools --toon")]
    Tools,
    /// Run one agent tool with its arguments as JSON on stdin, as the harness integrations do.
    #[command(after_help = "Example:\n  echo '{\"query\": \"cmake cannot find hdf5\"}' | rkb tool rkb_search")]
    Tool {
        /// rkb_search, rkb_show, rkb_add or rkb_note.
        name: String,
    },
    /// Save a short finding to the inbox, to turn into a lesson later with /rkb-distill. Changes no lesson.
    #[command(after_help = "Example:\n  rkb note \"the linker wants -lz after -lhdf5 on tuolumne\"\n  echo \"...\" | rkb note")]
    Note {
        /// The note. Default: stdin.
        words: Vec<String>,
    },
    /// List the inbox: notes and session extracts waiting to become lessons.
    #[command(after_help = "Example:\n  rkb inbox\n  rkb inbox show 0a1b2c3d4e\n  rkb inbox done 0a1b2c3d4e")]
    Inbox {
        #[command(subcommand)]
        action: Option<InboxCmd>,
    },
    /// Download or check model files for reranking.
    #[command(after_help = "Example:\n  rkb models fetch")]
    Models {
        #[command(subcommand)]
        action: ModelsCmd,
    },
    /// Build or preview a static site of the lessons the `web` sink allows, with rs-web.
    #[command(after_help = "Example:\n  rkb site build --out ~/site\n  rkb site serve\n  rkb site init\n  rkb site ci")]
    Site {
        #[command(subcommand)]
        action: SiteCmd,
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

#[derive(Subcommand)]
enum InboxCmd {
    /// Print one inbox item in full.
    #[command(after_help = "Example:\n  rkb inbox show 0a1b2c3d4e")]
    Show { id: String },
    /// Remove items that became lessons or held nothing worth one.
    #[command(after_help = "Example:\n  rkb inbox done 0a1b2c3d4e 1b2c3d4e5f")]
    Done {
        #[arg(required = true, num_args = 1..)]
        ids: Vec<String>,
    },
}

#[derive(Subcommand)]
enum SiteCmd {
    /// Build the site with rs-web (downloaded when missing), check it, and write it to --out.
    #[command(after_help = "Example:\n  rkb site build\n  rkb site build --out ~/site\n  rkb site build --no-ask --out dist")]
    Build {
        /// Where the built site goes. Default: $XDG_CACHE_HOME/rkb/site/dist.
        #[arg(long, value_name = "DIR")]
        out: Option<String>,
        /// Fail instead of asking about a first publication, and never write the knowledge base. For CI.
        #[arg(long)]
        no_ask: bool,
    },
    /// Stage the site and run `rs-web serve` on it for a live preview. Writes no output folder.
    #[command(after_help = "Example:\n  rkb site serve --port 8080")]
    Serve {
        #[arg(long, default_value_t = 3000)]
        port: u16,
    },
    /// Copy the built-in rs-web template to $RKB_HOME/site/ to customize it. Never overwrites a file.
    #[command(after_help = "Example:\n  rkb site init")]
    Init,
    /// Write .github/workflows/site.yml, which builds the site on push and deploys it to Cloudflare Pages. Never overwrites it.
    #[command(after_help = "Example:\n  rkb site ci")]
    Ci,
}

#[derive(Subcommand)]
enum ModelsCmd {
    /// Download the pinned Laya files (843 MB) and check each against its SHA-256. Skips files already present.
    #[command(after_help = "Example:\n  rkb models fetch\n  rkb models fetch laya")]
    Fetch {
        /// The model; only `laya` exists.
        #[arg(default_value = "laya", value_parser = ["laya"])]
        name: String,
    },
    /// Compile the GPU kernels for this rkb once (about 10 s), so searches can use the GPU. Needs no network.
    #[command(after_help = "Example:\n  rkb models warm")]
    Warm {
        #[arg(default_value = "laya", value_parser = ["laya"])]
        name: String,
    },
}

/// Turns a clap error into rkb's error record on stdout, so an agent can correct the call in one step.
fn parse_error(args: &[String], e: clap::Error) -> ExitCode {
    use clap::error::{ContextKind, ContextValue, ErrorKind};
    if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
        let _ = e.print();
        return ExitCode::SUCCESS;
    }
    let flag = args
        .windows(2)
        .find(|w| w[0] == "--format")
        .map(|w| w[1].clone())
        .or_else(|| args.iter().find_map(|a| a.strip_prefix("--format=").map(str::to_string)));
    let flag = flag.and_then(|f| <Format as clap::ValueEnum>::from_str(&f, true).ok());
    let flag = if args.iter().any(|a| a == "--toon") { Some(Format::Toon) } else { flag };
    let format = output::resolve(flag).unwrap_or(Format::Human);
    let mut cmd = Cli::command();
    cmd.build();
    let mut path = vec!["rkb".to_string()];
    for a in args.iter().skip(1).filter(|a| !a.starts_with('-')) {
        let Some(sub) = cmd.find_subcommand(a).cloned() else { break };
        cmd = sub;
        path.push(a.clone());
    }
    let path = path.join(" ");
    let rendered = e.render().to_string();
    let message = rendered.lines().next().unwrap_or("").trim_start_matches("error: ").to_string();
    let wants_command = matches!(
        e.kind(),
        ErrorKind::InvalidSubcommand | ErrorKind::MissingSubcommand | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    );
    let fix = if wants_command {
        let suggested = match e.get(ContextKind::SuggestedSubcommand) {
            Some(ContextValue::String(s)) => format!("did you mean `{path} {s}`? "),
            Some(ContextValue::Strings(v)) if !v.is_empty() => format!("did you mean `{path} {}`? ", v[0]),
            _ => String::new(),
        };
        let names: Vec<&str> = cmd.get_subcommands().map(|c| c.get_name()).filter(|n| *n != "help").collect();
        format!("{suggested}commands of `{path}`: {}", names.join(", "))
    } else {
        let (global, own): (Vec<_>, Vec<_>) = cmd.get_arguments().filter(|a| a.get_long() != Some("help")).partition(|a| a.is_global_set());
        let names = |args: &[&clap::Arg]| -> Vec<String> {
            args.iter()
                .map(|a| match a.get_long() {
                    Some(l) => format!("--{l}"),
                    None => format!("<{}>", a.get_id().as_str().to_lowercase()),
                })
                .collect()
        };
        let own = names(&own);
        let own = if own.is_empty() { "none".to_string() } else { own.join(", ") };
        format!("valid for `{path}`: {own}; on every command: {}; `{path} --help` has examples", names(&global).join(", "))
    };
    let message = if wants_command && e.kind() != ErrorKind::InvalidSubcommand { format!("`{path}` needs a command") } else { message };
    output::print_error(format, &CliError::new(output::ErrorCode::Usage, message, fix));
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if let [_, v] = args.as_slice()
        && matches!(v.as_str(), "-v" | "-V" | "--version")
    {
        println!("rkb {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let cli = match Cli::try_parse_from(&args) {
        Ok(c) => c,
        Err(e) => return parse_error(&args, e),
    };
    if let Some(Cmd::Mcp) = &cli.cmd {
        mcp::serve();
        return ExitCode::SUCCESS;
    }
    if let Some(Cmd::Tool { name }) = &cli.cmd {
        let (text, exit) = agent::from_stdin(name);
        println!("{text}");
        return ExitCode::from(exit);
    }
    if let Some(Cmd::Hook { event, harness }) = &cli.cmd {
        hook::run(event, harness);
        return ExitCode::SUCCESS;
    }
    if cli.toon && cli.format.is_some_and(|f| f != Format::Toon) {
        let e = CliError::new(output::ErrorCode::Usage, "--toon and --format ask for different formats", "pass one of them");
        output::print_error(Format::Toon, &e);
        return ExitCode::from(2);
    }
    let format = match output::resolve(if cli.toon { Some(Format::Toon) } else { cli.format }) {
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
    let result = run(cli.cmd, format, &hints, &with).and_then(|out| match writes::ask_now(format, &out) {
        Some(choice) => {
            run(Some(Cmd::Confirm { request: out.data["request"].as_str().unwrap_or_default().to_string(), choice }), format, &hints, &with)
        }
        None => Ok(out),
    });
    match result {
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
    let needs_place = matches!(
        cmd,
        None | Some(
            Cmd::Context
                | Cmd::Doctor { .. }
                | Cmd::Add { .. }
                | Cmd::Show { .. }
                | Cmd::Search { .. }
                | Cmd::Approve { .. }
                | Cmd::Verify { .. }
                | Cmd::Confirm { .. }
        )
    );
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
        Cmd::Search { query, literal, regex, all, status, limit, rerank, no_model } => {
            kb::open(&root)?;
            let mode = match (query, literal, regex) {
                (_, Some(t), _) => rkb_core::search::Mode::Literal(t),
                (_, _, Some(p)) => rkb_core::search::Mode::Regex(p),
                (Some(q), _, _) => rkb_core::search::Mode::Ranked(q),
                _ => unreachable!("clap requires one"),
            };
            rkb_core::facts::refresh(&root, &env.place.clone().unwrap_or_default());
            let facts = rkb_core::conditions::Facts::gather(&root, &env.place.clone().unwrap_or_default(), with);
            let opts = rkb_core::search::Options { all, every_status: status.is_some(), limit, probes: rkb_core::search::ProbeMode::Run };
            let only = if no_model { Some(rkb_core::rerank::BM25.to_string()) } else { rerank };
            searching::search(&env, &facts, mode, opts, only.as_deref())
        }
        Cmd::Find { words, limit } => {
            kb::open(&root)?;
            searching::find(&env, &words.join(" "), limit)
        }
        Cmd::Eval { queries, min_recall, rerank, recall_sweep } => {
            kb::open(&root)?;
            searching::eval(&env, queries, min_recall, &rerank, recall_sweep)
        }
        Cmd::Dupes { min } => {
            kb::open(&root)?;
            graphing::dupes(&env, min)
        }
        Cmd::Related { id, limit } => {
            kb::open(&root)?;
            graphing::related(&env, &id, limit)
        }
        Cmd::Graph { clusters, orphans, dot, mermaid } => {
            kb::open(&root)?;
            let view = match (clusters, orphans, dot, mermaid) {
                (true, ..) => graphing::View::Clusters,
                (_, true, ..) => graphing::View::Orphans,
                (_, _, true, _) => graphing::View::Dot,
                (.., true) => graphing::View::Mermaid,
                _ => graphing::View::Counts,
            };
            graphing::graph(&env, view)
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
        Cmd::Add { topic, kind, template, text, assets } => writes::add(&env, topic, kind, template, text, assets),
        Cmd::Edit { id, base, assets, text } => writes::edit(&env, id, base, assets, text),
        Cmd::Flag { id, reason } => writes::flag(&env, id, reason),
        Cmd::Review { folder, signals } => {
            kb::open(&root)?;
            if signals {
                return Ok(session::signals(&rkb_core::review::signals(&root, &env.state), colored));
            }
            let c = rkb_core::review::review(&root, &env.state, folder.as_deref())?;
            Ok(session::review(&c, colored))
        }
        Cmd::Supersede { id, by, reason } => writes::lifecycle(&env, rkb_core::request::Action::Supersede { id, by, reason }),
        Cmd::Archive { target, reason } => writes::lifecycle(&env, rkb_core::request::Action::Archive { target, reason }),
        Cmd::Move { id, folder } => writes::lifecycle(&env, rkb_core::request::Action::Move { id, folder }),
        Cmd::Rename { id, slug } => writes::lifecycle(&env, rkb_core::request::Action::Rename { id, slug }),
        Cmd::Approve { id, kind } => {
            kb::open(&root)?;
            let o = if let Some(key) = id.strip_prefix("facts.") {
                match rkb_core::verify::approve_fact_request(&env.ctx(), key)? {
                    Some(req) => rkb_core::write::Outcome::NeedsUser(req),
                    None => rkb_core::write::Outcome::Info(format!("The fact command {id} is already approved here")),
                }
            } else {
                let kind = kind.ok_or_else(|| {
                    CliError::new(
                        output::ErrorCode::Usage,
                        "rkb approve needs check or probe for a lesson",
                        "run `rkb approve <id> check`, or `rkb approve facts.<key>`",
                    )
                })?;
                let kind = rkb_core::script::Kind::parse(&kind).expect("clap checks the kind");
                match rkb_core::verify::approve_request(&env.ctx(), &id, kind, false)? {
                    Some(req) => rkb_core::write::Outcome::NeedsUser(req),
                    None => rkb_core::write::Outcome::Info(format!("The {} script of {id} is already approved here", kind.name())),
                }
            };
            Ok(writes::outcome(&env, o))
        }
        Cmd::Verify { id, auto } => {
            kb::open(&root)?;
            if auto {
                let s = rkb_core::verify::auto(&env.ctx())?;
                return Ok(session::verify_summary(&s, colored));
            }
            let (_, o) = rkb_core::verify::one(&env.ctx(), &id.expect("clap requires an id"), false)?;
            Ok(writes::outcome(&env, o))
        }
        Cmd::Unarchive { id } => writes::lifecycle(&env, rkb_core::request::Action::Unarchive { id }),
        Cmd::Used { id, failed, reason, session, .. } => writes::used(&env, id, failed, reason, session),
        Cmd::Log { id } => writes::log(&env, id),
        Cmd::Confirm { request, choice } => writes::confirm(&env, request, choice),
        Cmd::Hook { .. } => unreachable!("main runs hooks first"),
        Cmd::Import { dir } => writes::import(&env, &dir),
        Cmd::Note { words } => inbox::note(&env, words),
        Cmd::Tools => Ok(agent::list()),
        Cmd::Tool { .. } | Cmd::Mcp => unreachable!("main runs tools and the MCP server first"),
        Cmd::Inbox { action: None } => inbox::list(&env),
        Cmd::Inbox { action: Some(InboxCmd::Show { id }) } => inbox::show(&env, &id),
        Cmd::Inbox { action: Some(InboxCmd::Done { ids }) } => inbox::done(&env, &ids),
        Cmd::Models { action: ModelsCmd::Fetch { .. } } => models_fetch(),
        Cmd::Models { action: ModelsCmd::Warm { .. } } => models_warm(),
        Cmd::Site { action: SiteCmd::Build { out, no_ask } } => site::run(&env, out, None, &site::Approved::default(), no_ask),
        Cmd::Site { action: SiteCmd::Serve { port } } => site::run(&env, None, Some(port), &site::Approved::default(), false),
        Cmd::Site { action: SiteCmd::Init } => site::init(&env),
        Cmd::Site { action: SiteCmd::Ci } => site::ci(&env),
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
            let counts = usage::counts(&root).remove(&fm.id).unwrap_or_default();
            let (worked, failed) = (counts.last_worked.clone(), counts.last_failed.clone());
            let uses = format!(
                "hash {hash}  worked {} (last {})  failed {} (last {})  injected {}",
                counts.worked,
                worked.as_deref().unwrap_or("never"),
                counts.failed,
                failed.as_deref().unwrap_or("never"),
                counts.injected
            );
            let place = env.place.clone().unwrap_or_default();
            rkb_core::facts::refresh(&root, &place);
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
            let g = rkb_core::graph::Graph::load(&root)?;
            let (related, related_text) = match g.find(&fm.id) {
                Some(i) => graphing::related_of(&g, i, 3, colored),
                None => (vec![], String::new()),
            };
            let mut human = format!(
                "{}\n{}  {}  {}\n{}\n{}\n\n{}",
                paint(colored, "1", &title),
                paint(colored, "2", &lesson.path),
                paint(colored, "2", &format!("{:?}", fm.status).to_lowercase()),
                paint(colored, "2", &format!("verified {} ({age}, {:?})", fm.verified, fm.verified_how).to_lowercase()),
                paint(colored, "2", &uses),
                applies_text,
                text
            );
            if !related.is_empty() {
                human.push_str(&format!("\n{}{related_text}", paint(colored, "1", "related")));
            }
            let data = json!({
                "id": fm.id,
                "path": lesson.path,
                "title": title,
                "hash": hash,
                "last_worked": worked,
                "last_failed": failed,
                "usage": counts,
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
                "related": related,
            });
            Ok(Output { data, human, exit: 0, raw: false })
        }
    }
}

#[cfg(feature = "laya")]
fn models_fetch() -> Result<Output, CliError> {
    let dir = rkb_rerank::files::default_dir();
    let base = std::env::var("RKB_MODELS_URL").unwrap_or_else(|_| "https://huggingface.co".into());
    let f = rkb_rerank::files::fetch(&dir, &base).map_err(|e| {
        CliError::new(output::ErrorCode::Io, e.to_string(), "check the network and the disk space, then run `rkb models fetch` again")
    })?;
    let mb = f.bytes as f64 / 1e6;
    let mut human = String::new();
    for name in &f.downloaded {
        human.push_str(&format!("{} {name}\n", paint(false, "32", "downloaded")));
    }
    for name in &f.present {
        human.push_str(&format!("present    {name}\n"));
    }
    human.push_str(&format!("{mb:.0} MB downloaded into {}; every file matches its pinned hash", dir.display()));
    let warm = rkb_rerank::warm(&dir).map_err(|e| {
        CliError::new(
            output::ErrorCode::Io,
            format!("warming the GPU kernels failed: {e:#}"),
            "run `rkb models warm` to try again; the CPU engine works without it",
        )
    })?;
    if let Some(t) = warm {
        human.push_str(&format!("\nGPU kernels compiled for this rkb in {:.1} s", t.as_secs_f64()));
    }
    let data = json!({
        "dir": dir.display().to_string(),
        "revision": rkb_rerank::files::REVISION,
        "downloaded": f.downloaded,
        "present": f.present,
        "bytes": f.bytes,
        "gpu_warm_s": warm.map(|t| (t.as_secs_f64() * 10.0).round() / 10.0),
        "help": ["Add \"laya\" to `chain` under [rerank] in kb.toml to rerank with it"],
    });
    Ok(Output { data, human, exit: 0, raw: false })
}

#[cfg(feature = "laya")]
fn models_warm() -> Result<Output, CliError> {
    let dir = rkb_rerank::files::default_dir();
    let t = rkb_rerank::warm(&dir).map_err(|e| {
        CliError::new(
            output::ErrorCode::Io,
            format!("warming the GPU kernels failed: {e:#}"),
            "run `rkb models fetch` if the files are missing; the CPU engine works without the GPU",
        )
    })?;
    let (human, data) = match t {
        Some(t) => (
            format!("GPU kernels compiled for this rkb in {:.1} s; searches can use the GPU now", t.as_secs_f64()),
            json!({ "gpu": true, "seconds": (t.as_secs_f64() * 10.0).round() / 10.0 }),
        ),
        None => ("This rkb has no GPU engine; searches use the CPU".to_string(), json!({ "gpu": false })),
    };
    Ok(Output { data, human, exit: 0, raw: false })
}

#[cfg(not(feature = "laya"))]
fn models_warm() -> Result<Output, CliError> {
    models_fetch()
}

#[cfg(not(feature = "laya"))]
fn models_fetch() -> Result<Output, CliError> {
    Err(CliError::new(
        output::ErrorCode::Usage,
        "this rkb was built without the laya feature",
        "install an rkb built with the default features",
    ))
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
