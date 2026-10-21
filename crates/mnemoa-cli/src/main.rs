//! `mnemoa`: keep repo memory for coding agents small, true and enforced.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context as _, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use mnemoa_core::config::Config;
use mnemoa_core::report::{self, Format};
use mnemoa_core::rules;
use mnemoa_core::scan::{self, BASELINE_FILE, Baseline, ScanOptions};
use mnemoa_core::{Target, VERSION};

mod remote;

#[derive(Parser, Debug)]
#[command(name = "mnemoa", version = VERSION, about = "Stateless agents. Durable repos.", long_about = None)]
struct Cli {
    #[command(flatten)]
    global: Global,
    #[command(subcommand)]
    command: Command,
}

#[derive(Args, Debug, Clone)]
struct Global {
    /// Config file (default: <path>/.mnemoa.toml).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Comma-separated targets: claude-code, codex, cursor, copilot, kiro, cline, gemini.
    #[arg(long, global = true, value_delimiter = ',')]
    targets: Option<Vec<String>>,
    /// Color output.
    #[arg(long, global = true, value_enum, default_value_t = ColorChoice::Auto)]
    color: ColorChoice,
    /// Send the command to a running mnemoa-api (paths are relative to its MNEMOA_ROOT).
    #[arg(long, global = true, env = "MNEMOA_SERVER")]
    server: Option<String>,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Lint agent context files (rules MNA001–MNA013).
    Scan(ScanArgs),
    /// Check docs and plans against the code (paths, symbols, commands, tasks).
    Drift(DriftArgs),
    /// Show always-loaded context per target agent.
    Budget(BudgetArgs),
    /// Turn enforceable prose rules into executable checks (dry run unless --apply).
    Promote(PromoteArgs),
    /// Remove rules whose retire-when condition is met (dry run unless --apply).
    Retire(RetireArgs),
    /// One prune plan: delete, move, promote, retire and fix, with before/after token cost.
    Forget(ForgetArgs),
    /// Create minimal AGENTS.md, CLAUDE.md and .mnemoa.toml (never overwrites).
    Init {
        /// Repository root.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Also add a GitHub workflow that runs mnemoa.
        #[arg(long)]
        ci: bool,
    },
    /// List all rules.
    Rules {
        /// Output format: pretty or json.
        #[arg(long, default_value = "pretty")]
        format: String,
    },
    /// Explain one rule.
    Explain {
        /// Rule ID, for example MNA006.
        id: String,
    },
}

#[derive(Args, Debug)]
struct ScanArgs {
    /// Repository root.
    #[arg(default_value = ".")]
    path: PathBuf,
    /// Output format.
    #[arg(long, short, default_value = "pretty")]
    format: String,
    /// Run only these rules (comma-separated IDs). Enables opt-in rules.
    #[arg(long, value_delimiter = ',')]
    rules: Option<Vec<String>>,
    /// Fail on warnings as well as errors.
    #[arg(long)]
    strict: bool,
    /// Baseline file of known findings to hide.
    #[arg(long)]
    baseline: Option<PathBuf>,
    /// Write all current findings to the baseline file and exit 0.
    #[arg(long)]
    write_baseline: bool,
    /// Skip reading git history (disables MNA005).
    #[arg(long)]
    no_history: bool,
}

#[derive(Args, Debug)]
struct DriftArgs {
    /// Repository root.
    #[arg(default_value = ".")]
    path: PathBuf,
    /// Base revision: references to files or symbols removed or renamed since it fail.
    #[arg(long)]
    base: Option<String>,
    /// Output format: pretty, json or github.
    #[arg(long, short, default_value = "pretty")]
    format: String,
    /// Fail on warnings as well as errors.
    #[arg(long)]
    strict: bool,
}

#[derive(Args, Debug)]
struct BudgetArgs {
    /// Repository root.
    #[arg(default_value = ".")]
    path: PathBuf,
    /// Output format: pretty or json.
    #[arg(long, short, default_value = "pretty")]
    format: String,
    /// Exit 1 if any target is over `rules.token_budget`.
    #[arg(long)]
    check: bool,
}

#[derive(Args, Debug)]
struct PromoteArgs {
    /// Repository root.
    #[arg(default_value = ".")]
    path: PathBuf,
    /// Write artifacts, the ledger and the doc edits.
    #[arg(long)]
    apply: bool,
    /// Ask the configured LLM to draft ast-grep rules for prose the catalog cannot match.
    #[arg(long)]
    llm: bool,
    /// Output format: pretty or json.
    #[arg(long, short, default_value = "pretty")]
    format: String,
}

#[derive(Args, Debug)]
struct RetireArgs {
    /// Repository root.
    #[arg(default_value = ".")]
    path: PathBuf,
    /// Delete the rules whose condition is met.
    #[arg(long)]
    apply: bool,
    /// Output format: pretty or json.
    #[arg(long, short, default_value = "pretty")]
    format: String,
}

#[derive(Args, Debug)]
struct ForgetArgs {
    /// Repository root.
    #[arg(default_value = ".")]
    path: PathBuf,
    /// Write the plan (needs --yes, or confirmation on a terminal).
    #[arg(long)]
    apply: bool,
    /// Do not ask for confirmation.
    #[arg(long, short)]
    yes: bool,
    /// Output format: pretty or json.
    #[arg(long, short, default_value = "pretty")]
    format: String,
}

fn use_color(choice: ColorChoice) -> bool {
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => {
            std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
        }
    }
}

fn parse_targets(t: &Option<Vec<String>>) -> Result<Option<Vec<Target>>> {
    match t {
        None => Ok(None),
        Some(list) => list
            .iter()
            .map(|s| s.parse::<Target>().map_err(anyhow::Error::msg))
            .collect::<Result<Vec<_>>>()
            .map(Some),
    }
}

fn load_config(root: &Path, g: &Global) -> Result<Config> {
    let mut c = Config::load(root, g.config.as_deref()).context("loading config")?;
    if let Some(t) = parse_targets(&g.targets)? {
        c.targets = t;
    }
    Ok(c)
}

fn print(s: &str) -> Result<()> {
    let mut out = std::io::stdout().lock();
    match out.write_all(s.as_bytes()) {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        r => r.context("writing output"),
    }
}

fn cmd_scan(a: &ScanArgs, g: &Global) -> Result<i32> {
    if !a.path.is_dir() {
        bail!("`{}` is not a directory", a.path.display());
    }
    let config = load_config(&a.path, g)?;
    let format: Format = a.format.parse().map_err(anyhow::Error::msg)?;
    let baseline_path = a
        .baseline
        .clone()
        .unwrap_or_else(|| a.path.join(BASELINE_FILE));
    let baseline = if !a.write_baseline && baseline_path.is_file() {
        Some(Baseline::load(&baseline_path)?)
    } else {
        None
    };
    let opts = ScanOptions {
        targets: None,
        rules: a.rules.clone(),
        strict: a.strict.then_some(true),
        baseline,
        history: !a.no_history,
    };
    let report = scan::scan(&a.path, &config, &opts)?;
    if a.write_baseline {
        let b = Baseline::from_findings(&report.findings);
        b.save(&baseline_path)?;
        eprintln!(
            "wrote {} finding(s) to {}",
            b.findings.len(),
            baseline_path.display()
        );
        return Ok(0);
    }
    print(&report::render(&report, format, use_color(g.color)))?;
    Ok(report.exit_code())
}

fn cmd_drift(a: &DriftArgs, g: &Global) -> Result<i32> {
    if !a.path.is_dir() {
        bail!("`{}` is not a directory", a.path.display());
    }
    let config = load_config(&a.path, g)?;
    let format: Format = a.format.parse().map_err(anyhow::Error::msg)?;
    let ws = scan::load_workspace(&a.path, &config)?;
    let history = scan::open_history(&a.path, &config)?;
    if a.base.is_some() && history.is_none() {
        bail!("--base needs a git repository");
    }
    let report = mnemoa_core::drift::drift(
        &ws,
        &config,
        history
            .as_ref()
            .map(|h| h as &dyn mnemoa_core::history::History),
        &mnemoa_core::drift::DriftOptions {
            base: a.base.clone(),
        },
    )?;
    print(&report::render_drift(&report, format, use_color(g.color)))?;
    Ok(report.exit_code(a.strict))
}

fn cmd_budget(a: &BudgetArgs, g: &Global) -> Result<i32> {
    if !a.path.is_dir() {
        bail!("`{}` is not a directory", a.path.display());
    }
    let config = load_config(&a.path, g)?;
    let format: Format = a.format.parse().map_err(anyhow::Error::msg)?;
    let ws = scan::load_workspace(&a.path, &config)?;
    let report = mnemoa_core::budget::budget(&ws, &config.targets, mnemoa_core::Tokenizer::best())
        .with_token_budget(config.rules.token_budget);
    print(&report::render_budget(&report, format, use_color(g.color)))?;
    Ok(if a.check && report.over_budget() {
        1
    } else {
        0
    })
}

#[cfg(feature = "llm")]
fn drafter(config: &Config) -> Result<Box<dyn mnemoa_core::drafter::Drafter>> {
    Ok(Box::new(mnemoa_core::llm::HttpDrafter::new(&config.llm)?))
}

#[cfg(not(feature = "llm"))]
fn drafter(_config: &Config) -> Result<Box<dyn mnemoa_core::drafter::Drafter>> {
    bail!("this build has no LLM support; rebuild with `--features llm`")
}

fn cmd_promote(a: &PromoteArgs, g: &Global) -> Result<i32> {
    if !a.path.is_dir() {
        bail!("`{}` is not a directory", a.path.display());
    }
    let config = load_config(&a.path, g)?;
    let ws = scan::load_workspace(&a.path, &config)?;
    let ast_grep = mnemoa_core::drafter::find_ast_grep();
    let d = if a.llm { Some(drafter(&config)?) } else { None };
    let plan = mnemoa_core::promote::plan_with(&ws, &config, d.as_deref(), ast_grep.as_deref());
    if a.format == "json" {
        print(&(serde_json::to_string_pretty(&plan)? + "\n"))?;
    } else {
        let color = use_color(g.color);
        let mut s = String::new();
        for p in &plan.promotions {
            let status = match p.status {
                mnemoa_core::promote::Status::AlreadyEnforced => "already enforced",
                mnemoa_core::promote::Status::Emit => "emit check",
                mnemoa_core::promote::Status::Drafted => "drafted (validated)",
            };
            s.push_str(&format!(
                "{}:{}  {}  [{}] {}\n",
                p.source, p.line, p.rule, p.enforcer, status
            ));
            for art in &p.artifacts {
                let action = match art.action {
                    mnemoa_core::promote::Action::Create => "create",
                    mnemoa_core::promote::Action::Update => "update",
                    mnemoa_core::promote::Action::Manual => "manual",
                };
                s.push_str(&format!("    {action:<6} {}  {}\n", art.path, art.note));
            }
        }
        for u in &plan.unmatched {
            let note = match &u.draft {
                Some(d) => format!(" (draft {:?})", d.validation),
                None => String::new(),
            };
            s.push_str(&format!(
                "{}:{}  {}  [no catalog match]{note}\n",
                u.source, u.line, u.rule
            ));
        }
        if !plan.diff.is_empty() && !a.apply {
            s.push('\n');
            s.push_str(&colorize_diff(&plan.diff, color));
        }
        s.push_str(&format!(
            "\n{} promotion(s), {} unmatched; {} file(s) {}.\n",
            plan.promotions.len(),
            plan.unmatched.len(),
            plan.files_changed.len(),
            if a.apply {
                "written"
            } else {
                "would change (run with --apply)"
            }
        ));
        print(&s)?;
    }
    if a.apply {
        mnemoa_core::promote::apply(&ws, &plan)?;
    }
    Ok(0)
}

fn colorize_diff(diff: &str, color: bool) -> String {
    if !color {
        return diff.to_string();
    }
    diff.lines()
        .map(|l| {
            if l.starts_with("+++") || l.starts_with("---") {
                format!("\x1b[1m{l}\x1b[0m")
            } else if l.starts_with('+') {
                format!("\x1b[32m{l}\x1b[0m")
            } else if l.starts_with('-') {
                format!("\x1b[31m{l}\x1b[0m")
            } else if l.starts_with("@@") {
                format!("\x1b[36m{l}\x1b[0m")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn cmd_retire(a: &RetireArgs, g: &Global) -> Result<i32> {
    if !a.path.is_dir() {
        bail!("`{}` is not a directory", a.path.display());
    }
    let config = load_config(&a.path, g)?;
    let ws = scan::load_workspace(&a.path, &config)?;
    let ast_grep = mnemoa_core::drafter::find_ast_grep();
    let plan = mnemoa_core::retire::plan(
        &ws,
        &config,
        &mnemoa_core::ledger::today(),
        ast_grep.as_deref(),
    );
    if a.format == "json" {
        print(&(serde_json::to_string_pretty(&plan)? + "\n"))?;
    } else {
        let mut s = String::new();
        for c in &plan.candidates {
            let mark = if c.met && !c.protected && c.removable {
                "retire"
            } else if c.met {
                "keep (protected or nested)"
            } else {
                "keep"
            };
            s.push_str(&format!(
                "{}:{}  {}  [{}] {mark}: {}\n",
                c.source, c.line, c.rule, c.condition, c.reason
            ));
        }
        if !plan.diff.is_empty() && !a.apply {
            s.push('\n');
            s.push_str(&colorize_diff(&plan.diff, use_color(g.color)));
        }
        s.push_str(&format!(
            "\n{} rule(s) with a retire condition; {} {}.\n",
            plan.candidates.len(),
            plan.retiring().count(),
            if a.apply {
                "retired"
            } else {
                "would be retired (run with --apply)"
            }
        ));
        print(&s)?;
    }
    if a.apply {
        mnemoa_core::retire::apply(&ws, &plan)?;
    }
    Ok(0)
}

fn cmd_forget(a: &ForgetArgs, g: &Global) -> Result<i32> {
    if !a.path.is_dir() {
        bail!("`{}` is not a directory", a.path.display());
    }
    let config = load_config(&a.path, g)?;
    let ws = scan::load_workspace(&a.path, &config)?;
    let ast_grep = mnemoa_core::drafter::find_ast_grep();
    let plan = mnemoa_core::forget::plan(
        &ws,
        &config,
        &mnemoa_core::ledger::today(),
        ast_grep.as_deref(),
    );
    if a.format == "json" {
        print(&(serde_json::to_string_pretty(&plan)? + "\n"))?;
    } else {
        let color = use_color(g.color);
        let mut s = String::new();
        for act in &plan.actions {
            let kind = serde_json::to_value(act.kind)?
                .as_str()
                .unwrap_or_default()
                .to_string();
            s.push_str(&format!(
                "{}:{}  {:<13} {}\n",
                act.path, act.line, kind, act.summary
            ));
        }
        if plan.actions.is_empty() {
            s.push_str("Nothing to forget.\n");
        }
        s.push_str(&format!("\nAlways-loaded tokens ({}):\n", plan.tokenizer));
        for t in &plan.tokens {
            let delta = t.after as i64 - t.before as i64;
            s.push_str(&format!(
                "  {:<12} {:>6} -> {:>6}  ({delta:+})\n",
                t.target.as_str(),
                t.before,
                t.after
            ));
        }
        if !plan.diff.is_empty() && !a.apply {
            s.push('\n');
            s.push_str(&colorize_diff(&plan.diff, color));
        }
        print(&s)?;
    }
    if a.apply && !plan.files.is_empty() {
        if !a.yes {
            if !std::io::stdin().is_terminal() {
                bail!("--apply needs --yes when not running in a terminal");
            }
            eprint!("Write {} file(s)? [y/N] ", plan.files.len());
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer)?;
            if !matches!(answer.trim(), "y" | "Y" | "yes") {
                eprintln!("aborted");
                return Ok(1);
            }
        }
        let written = mnemoa_core::forget::apply(&ws, &plan)?;
        eprintln!("wrote {} file(s)", written.len());
    }
    Ok(0)
}

fn cmd_init(path: &Path, ci: bool) -> Result<i32> {
    if !path.is_dir() {
        bail!("`{}` is not a directory", path.display());
    }
    let ws = mnemoa_core::Workspace::load(path, &Default::default())?;
    let plan = mnemoa_core::init::plan(&ws, &mnemoa_core::init::InitOptions { ci });
    let written = mnemoa_core::init::apply(&ws, &plan)?;
    let mut s = String::new();
    for w in &written {
        s.push_str(&format!("created  {w}\n"));
    }
    for k in &plan.skipped {
        s.push_str(&format!("skipped  {k} (exists)\n"));
    }
    s.push_str("Next: edit the one-sentence description in AGENTS.md, then run `mnemoa scan`.\n");
    print(&s)?;
    Ok(0)
}

fn cmd_rules(format: &str) -> Result<i32> {
    if format == "json" {
        print(&(serde_json::to_string_pretty(&rules::RULES)? + "\n"))?;
        return Ok(0);
    }
    let mut s = String::new();
    for r in &rules::RULES {
        let opt = if r.opt_in { " (opt-in)" } else { "" };
        s.push_str(&format!(
            "{}  {:<26} {:<5}  {}{opt}\n",
            r.id,
            r.name,
            r.default_severity.as_str(),
            r.summary
        ));
    }
    print(&s)?;
    Ok(0)
}

fn cmd_explain(id: &str) -> Result<i32> {
    let Some(r) = rules::info(id) else {
        bail!("unknown rule `{id}`; run `mnemoa rules` for the list");
    };
    print(&format!(
        "{} {}\n\n{}\n\nWhy: {}\n\n{}\n\nDefault severity: {}{}{}\nResearch: {}\n",
        r.id,
        r.name,
        r.summary,
        r.rationale,
        r.explain,
        r.default_severity,
        if r.fixable {
            " · fixable by `mnemoa forget`"
        } else {
            ""
        },
        if r.opt_in { " · opt-in" } else { "" },
        r.link
    ))?;
    Ok(0)
}

fn run(cli: Cli) -> Result<i32> {
    if let Some(url) = &cli.global.server {
        return remote::run(url, &cli);
    }
    match &cli.command {
        Command::Scan(a) => cmd_scan(a, &cli.global),
        Command::Drift(a) => cmd_drift(a, &cli.global),
        Command::Budget(a) => cmd_budget(a, &cli.global),
        Command::Promote(a) => cmd_promote(a, &cli.global),
        Command::Retire(a) => cmd_retire(a, &cli.global),
        Command::Forget(a) => cmd_forget(a, &cli.global),
        Command::Init { path, ci } => cmd_init(path, *ci),
        Command::Rules { format } => cmd_rules(format),
        Command::Explain { id } => cmd_explain(id),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("mnemoa: error: {e:#}");
            ExitCode::from(2)
        }
    }
}
