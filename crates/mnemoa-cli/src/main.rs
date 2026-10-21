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
    Ok(report.exit_code(a.strict || config.scan.strict))
}

fn cmd_budget(a: &BudgetArgs, g: &Global) -> Result<i32> {
    if !a.path.is_dir() {
        bail!("`{}` is not a directory", a.path.display());
    }
    let config = load_config(&a.path, g)?;
    let format: Format = a.format.parse().map_err(anyhow::Error::msg)?;
    let ws = scan::load_workspace(&a.path, &config)?;
    let report = mnemoa_core::budget::budget(&ws, &config.targets, mnemoa_core::Tokenizer::best());
    print(&report::render_budget(&report, format, use_color(g.color)))?;
    let over = report
        .targets
        .iter()
        .any(|t| t.total_tokens > config.rules.token_budget || t.dropped_bytes.is_some());
    Ok(if a.check && over { 1 } else { 0 })
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
    match &cli.command {
        Command::Scan(a) => cmd_scan(a, &cli.global),
        Command::Drift(a) => cmd_drift(a, &cli.global),
        Command::Budget(a) => cmd_budget(a, &cli.global),
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
