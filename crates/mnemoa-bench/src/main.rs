//! `mnemoa-bench`: 12 benchmark suites for mnemoa-core.
//!
//! Usage: `mnemoa-bench [--filter TEXT] [--json] [--quick]`
//!
//! Each suite builds its input once (untimed), runs one warm-up, then repeats until it has run
//! for at least the time budget. Reported numbers come from this machine only.

use std::hint::black_box;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use mnemoa_core::catalog::Catalog;
use mnemoa_core::config::Config;
use mnemoa_core::diag::{Finding, Severity, Summary};
use mnemoa_core::doc::Doc;
use mnemoa_core::history::{GixHistory, History};
use mnemoa_core::report;
use mnemoa_core::rules::{self, Context};
use mnemoa_core::scan::ScanReport;
use mnemoa_core::{DiscoverOptions, Target, Tokenizer, Workspace, discover};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct Result1 {
    id: usize,
    name: String,
    iterations: u64,
    avg_ns: u128,
    min_ns: u128,
    max_ns: u128,
    /// Work units per second, based on the average.
    throughput: f64,
    unit: String,
}

#[derive(Debug, Serialize)]
struct Hardware {
    cpu: String,
    cores: usize,
    os: String,
    profile: String,
}

#[derive(Debug, Serialize)]
struct Output {
    version: String,
    hardware: Hardware,
    results: Vec<Result1>,
    skipped: Vec<String>,
}

/// The timed closure and the number of work units it processes per run.
type Prepared = (Box<dyn FnMut()>, f64);

struct Suite {
    id: usize,
    name: &'static str,
    unit: &'static str,
    /// Build input; returns the closure to time and the number of units per run.
    setup: fn(&Path) -> Result<Prepared>,
}

fn hardware() -> Hardware {
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.trim().to_string())
        })
        .or_else(|| {
            Command::new("sysctl")
                .args(["-n", "machdep.cpu.brand_string"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "unknown".into());
    Hardware {
        cpu,
        cores: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
        os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        profile: if cfg!(debug_assertions) {
            "debug".into()
        } else {
            "release".into()
        },
    }
}

fn write(root: &Path, rel: &str, content: &str) -> Result<()> {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(p, content)?;
    Ok(())
}

/// A 1,000-line instruction document with every construct the parser handles.
fn big_doc() -> String {
    let mut s = String::from("---\npaths:\n  - src/**\n---\n# Project\n\n");
    let mut line = 6;
    let mut i = 0;
    while line < 1000 {
        match i % 10 {
            0 => s.push_str(&format!("## Section {i}\n\n")),
            1 => s.push_str(&format!(
                "- Always run `make test-{i}` before committing changes to module {i}.\n"
            )),
            2 => s.push_str(&format!(
                "- See [guide {i}](docs/guide-{i}.md) when editing `src/mod_{i}.rs`.\n"
            )),
            3 => s.push_str("<!-- why: flaky CI | seen: 3x | retire-when: date:2027-01-01 -->\n"),
            4 => s.push_str(&format!(
                "- Never call `.unwrap()` in `src/mod_{i}/`; use `?` with `anyhow::Context`.\n"
            )),
            5 => s.push_str(&format!(
                "Use pnpm for package {i}. The service starts on port 80{:02}.\n",
                i % 100
            )),
            6 => s.push_str(&format!(
                "1. Step one for task {i}\n2. Step two\n3. Step three\n"
            )),
            7 => s.push_str("```sh\ncargo build --release\nmake lint\n```\n"),
            8 => s.push_str(&format!("@docs/extra-{i}.md\n")),
            _ => s.push('\n'),
        }
        line = s.lines().count();
        i += 1;
    }
    s
}

fn suite_discovery(dir: &Path) -> Result<Prepared> {
    let root = dir.join("tree");
    for i in 0..10_000 {
        let rel = format!("pkg{:02}/src/dir{:02}/file{i}.rs", i % 50, i % 20);
        write(&root, &rel, "")?;
    }
    write(&root, "AGENTS.md", "# A\n")?;
    write(&root, "CLAUDE.md", "@AGENTS.md\n")?;
    write(&root, ".gitignore", "target/\n")?;
    Ok((
        Box::new(move || {
            black_box(discover(&root, &DiscoverOptions::default()).unwrap());
        }),
        10_003.0,
    ))
}

fn suite_parse(_: &Path) -> Result<Prepared> {
    let text = big_doc();
    let lines = text.lines().count() as f64;
    Ok((
        Box::new(move || {
            black_box(Doc::parse("AGENTS.md", &text));
        }),
        lines,
    ))
}

fn suite_tokens(_: &Path) -> Result<Prepared> {
    let text = big_doc();
    let lines = text.lines().count() as f64;
    let tok = Tokenizer::best();
    tok.count("warm up the tokenizer");
    Ok((
        Box::new(move || {
            black_box(tok.count(&text));
        }),
        lines,
    ))
}

fn load(root: &Path) -> Workspace {
    Workspace::load(root, &DiscoverOptions::default()).unwrap()
}

fn suite_rule_pass(dir: &Path) -> Result<Prepared> {
    let root = dir.join("rules");
    let doc: String = big_doc().lines().take(200).collect::<Vec<_>>().join("\n");
    write(&root, "AGENTS.md", &doc)?;
    write(&root, "Makefile", "test-1:\n\tx\nlint:\n\tx\n")?;
    write(&root, ".editorconfig", "[*]\nindent_style = space\n")?;
    let ws = load(&root);
    let config = Config::default();
    Ok((
        Box::new(move || {
            let cx = Context::new(&ws, &config, &config.targets);
            black_box(rules::run(&cx));
        }),
        1.0,
    ))
}

fn suite_references(dir: &Path) -> Result<Prepared> {
    let root = dir.join("refs");
    let mut doc = String::new();
    for i in 0..500 {
        if i % 2 == 0 {
            doc.push_str(&format!(
                "- Read [doc {i}](docs/d{i}.md) before changing area {i}.\n"
            ));
            if i % 4 == 0 {
                write(&root, &format!("docs/d{i}.md"), "x")?;
            }
        } else {
            doc.push_str(&format!(
                "- Code for area {i} lives in `src/area{i}/mod.rs`.\n"
            ));
            if i % 3 == 0 {
                write(&root, &format!("src/area{i}/mod.rs"), "")?;
            }
        }
    }
    write(&root, "AGENTS.md", &doc)?;
    let ws = load(&root);
    let config = Config::default();
    let rule = rules::references::BrokenReference;
    Ok((
        Box::new(move || {
            let cx = Context::new(&ws, &config, &config.targets);
            let mut out = Vec::new();
            rules::Check::check(&rule, &cx, &mut out);
            black_box(out);
        }),
        500.0,
    ))
}

fn suite_commands(dir: &Path) -> Result<Prepared> {
    let root = dir.join("cmds");
    let mut make = String::new();
    let mut scripts = Vec::new();
    for i in 0..100 {
        make.push_str(&format!("target{i}:\n\techo {i}\n"));
        scripts.push(format!("\"script{i}\": \"echo {i}\""));
    }
    write(&root, "Makefile", &make)?;
    write(
        &root,
        "package.json",
        &format!("{{\"scripts\": {{{}}}}}", scripts.join(",")),
    )?;
    let mut doc = String::new();
    for i in 0..200 {
        doc.push_str(&format!(
            "- Run `make target{}` or `npm run script{}`.\n",
            i % 130,
            i % 120
        ));
    }
    write(&root, "AGENTS.md", &doc)?;
    let ws = load(&root);
    let config = Config::default();
    let rule = rules::commands::StaleCommand;
    Ok((
        Box::new(move || {
            let cx = Context::new(&ws, &config, &config.targets);
            let mut out = Vec::new();
            rules::Check::check(&rule, &cx, &mut out);
            black_box(out);
        }),
        400.0,
    ))
}

fn suite_lint_leakage(dir: &Path) -> Result<Prepared> {
    let root = dir.join("lint");
    write(
        &root,
        ".editorconfig",
        "[*]\nindent_style = space\nindent_size = 2\nmax_line_length = 100\n",
    )?;
    write(&root, ".prettierrc", "{}")?;
    write(
        &root,
        "ruff.toml",
        "[lint]\nselect = [\"E\", \"F\", \"I\"]\n",
    )?;
    let ws = load(&root);
    let configured = mnemoa_core::enforcers::detect(&ws);
    let catalog = Catalog::builtin();
    let samples = [
        "Use 2 spaces for indentation.",
        "Keep lines under 100 characters.",
        "Always use single quotes in TypeScript.",
        "Sort imports alphabetically.",
        "Never leave unused imports.",
        "Write tests for every bug fix.",
        "Prefer small pull requests.",
        "Format all code with Prettier.",
    ];
    let rules_text: Vec<String> = (0..200)
        .map(|i| format!("{} ({i})", samples[i % samples.len()]))
        .collect();
    Ok((
        Box::new(move || {
            for r in &rules_text {
                for t in catalog.matches(r) {
                    black_box(t.covered_by(r, &configured));
                }
            }
        }),
        200.0,
    ))
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn suite_history(dir: &Path) -> Result<Prepared> {
    anyhow::ensure!(git_available(), "git is not installed");
    let root = dir.join("history");
    std::fs::create_dir_all(&root)?;
    let git = |args: &[&str]| -> Result<()> {
        let st = Command::new("git").args(args).current_dir(&root).output()?;
        anyhow::ensure!(
            st.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&st.stderr)
        );
        Ok(())
    };
    git(&["init", "-q", "-b", "main"])?;
    // Build 2,000 commits quickly with fast-import.
    let mut stream = String::new();
    for i in 0..2000 {
        let path = if i % 25 == 0 {
            "AGENTS.md".to_string()
        } else {
            format!("src/file{}.rs", i % 200)
        };
        let content = format!("// change {i}\n");
        stream.push_str(&format!(
            "commit refs/heads/main\nmark :{m}\ncommitter Bench <b@example.com> {t} +0000\ndata 3\nc{n}\n",
            m = i + 1,
            t = 1_700_000_000 + i * 60,
            n = i % 10
        ));
        if i > 0 {
            stream.push_str(&format!("from :{}\n", i));
        }
        stream.push_str(&format!(
            "M 100644 inline {path}\ndata {}\n{content}\n",
            content.len()
        ));
    }
    let mut child = Command::new("git")
        .args(["fast-import", "--quiet"])
        .current_dir(&root)
        .stdin(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .context("stdin")?
        .write_all(stream.as_bytes())?;
    anyhow::ensure!(child.wait()?.success(), "git fast-import failed");
    git(&["checkout", "-q", "main"])?;
    Ok((
        Box::new(move || {
            let h = GixHistory::open(&root, 5000).unwrap().unwrap();
            assert_eq!(h.commits().len(), 2000);
            black_box(h);
        }),
        2000.0,
    ))
}

fn suite_plan_drift(dir: &Path) -> Result<Prepared> {
    let root = dir.join("plans");
    let mut tasks = String::from("# Tasks\n\n");
    for i in 1..=300 {
        let done = if i % 3 == 0 { "x" } else { " " };
        tasks.push_str(&format!(
            "- [{done}] T{i:03} [P] [US{}] Implement feature in src/feature_{i}.py\n",
            i % 5 + 1
        ));
        if i % 2 == 0 {
            write(&root, &format!("src/feature_{i}.py"), "")?;
        }
    }
    write(&root, "specs/001-big/tasks.md", &tasks)?;
    let ws = load(&root);
    let config = Config::default();
    Ok((
        Box::new(move || {
            black_box(mnemoa_core::plans::plan_drift(&ws, &config, None));
        }),
        300.0,
    ))
}

fn suite_promotion(dir: &Path) -> Result<Prepared> {
    let root = dir.join("promote");
    write(&root, "Cargo.toml", "[package]\nname = \"x\"\n")?;
    write(&root, "src/lib.rs", "pub fn a() {}\n")?;
    write(&root, "src/api/mod.rs", "")?;
    write(&root, "src/internal/mod.rs", "")?;
    let mut doc = String::new();
    let lines = [
        "Use 4 spaces for indentation.",
        "Never call `.unwrap()` in `src/`.",
        "Always run `make lint` before committing.",
        "`src/api/` must not import from `src/internal/`.",
        "Keep lines under 100 characters.",
        "Remove trailing whitespace.",
        "Never use `eval()`.",
        "Write clear commit messages.",
    ];
    for i in 0..40 {
        doc.push_str(&format!("- {} ({i})\n", lines[i % lines.len()]));
    }
    write(&root, "AGENTS.md", &doc)?;
    let ws = load(&root);
    let config = Config::default();
    Ok((
        Box::new(move || {
            black_box(mnemoa_core::promote::plan(&ws, &config));
        }),
        40.0,
    ))
}

fn suite_sarif(_: &Path) -> Result<Prepared> {
    let doc = Doc::parse("AGENTS.md", &big_doc());
    let mut findings = Vec::new();
    for i in 0..1000 {
        let id = &rules::RULES[i % rules::RULES.len()];
        let start = (i * 37) % (doc.text.len() - 10);
        let start = (0..=start)
            .rev()
            .find(|s| doc.text.is_char_boundary(*s))
            .unwrap_or(0);
        findings.push(Finding::at(
            id.id,
            id.name,
            Severity::Warn,
            &doc,
            start..start + 5,
            format!("finding {i}"),
        ));
    }
    let report = ScanReport {
        version: mnemoa_core::VERSION.into(),
        root: ".".into(),
        targets: vec![Target::ClaudeCode],
        config_source: "defaults".into(),
        strict: false,
        summary: Summary::of(&findings),
        findings,
        baselined: 0,
        files: vec!["AGENTS.md".into()],
    };
    Ok((
        Box::new(move || {
            black_box(report::render(&report, report::Format::Sarif, false));
        }),
        1000.0,
    ))
}

/// A generated monorepo with several packages and agent files of every kind.
fn monorepo(root: &Path) -> Result<()> {
    write(root, "Makefile", "lint:\n\tx\ntest:\n\ty\n")?;
    write(
        root,
        ".editorconfig",
        "root = true\n[*]\nindent_style = space\nindent_size = 2\n",
    )?;
    write(
        root,
        "package.json",
        r#"{"scripts":{"build":"tsc","test":"vitest"},"devDependencies":{"typescript":"5","vitest":"1","react":"18"}}"#,
    )?;
    write(root, "pnpm-lock.yaml", "")?;
    write(
        root,
        "AGENTS.md",
        "# Monorepo\n\n- Use 2 spaces for indentation.\n- Run `make lint` before committing.\n- Read [arch](docs/architecture.md)\n\n## Stack\n\n- TypeScript\n- React\n- Vitest\n\n```\n.\n├── packages/\n│   ├── web/\n│   └── api/\n└── docs/\n```\n\n## Adding a new package\n\n1. Create `packages/<name>/package.json` with the workspace scripts and the shared tsconfig.\n2. Add the package to the root workspace list and run the install to link it.\n3. Add a CODEOWNERS entry and a short README describing what the package owns.\n4. Wire the package into CI by adding it to the build matrix.\n\n<!-- why: launch | retire-when: date:2026-01-01 -->\n- Freeze the public API until launch.\n- Never commit secrets.\n",
    )?;
    write(
        root,
        "CLAUDE.md",
        "- Use 2 spaces for indentation.\n- Prefer plan mode for refactors.\n",
    )?;
    write(root, "docs/architecture.md", "# Architecture\n")?;
    for pkg in ["web", "api", "shared", "cli"] {
        write(
            root,
            &format!("packages/{pkg}/AGENTS.md"),
            &format!("# {pkg}\n\n- Run `pnpm run test` in this package.\n"),
        )?;
        for i in 0..50 {
            write(
                root,
                &format!("packages/{pkg}/src/file{i}.ts"),
                &format!("export function f{i}() {{ return {i}; }}\n"),
            )?;
        }
    }
    write(
        root,
        ".cursor/rules/style.mdc",
        "---\nalwaysApply: true\n---\n- Prefer named exports.\n",
    )?;
    write(
        root,
        "specs/001-auth/tasks.md",
        "- [x] T001 Create packages/api/src/auth.ts\n- [ ] T002 [US1] Add tests\n",
    )?;
    Ok(())
}

fn suite_forget(dir: &Path) -> Result<Prepared> {
    let root = dir.join("monorepo");
    monorepo(&root)?;
    let config = Config::default();
    Ok((
        Box::new(move || {
            let ws = load(&root);
            black_box(mnemoa_core::forget::plan(&ws, &config, "2026-10-21", None));
        }),
        1.0,
    ))
}

const SUITES: &[Suite] = &[
    Suite {
        id: 1,
        name: "discovery walk (10k files)",
        unit: "files",
        setup: suite_discovery,
    },
    Suite {
        id: 2,
        name: "markdown parse (1k lines)",
        unit: "lines",
        setup: suite_parse,
    },
    Suite {
        id: 3,
        name: "token estimate (1k lines)",
        unit: "lines",
        setup: suite_tokens,
    },
    Suite {
        id: 4,
        name: "full rule pass (200-line file)",
        unit: "passes",
        setup: suite_rule_pass,
    },
    Suite {
        id: 5,
        name: "reference resolution (500 refs)",
        unit: "refs",
        setup: suite_references,
    },
    Suite {
        id: 6,
        name: "stale-command lookup (400 cmds)",
        unit: "cmds",
        setup: suite_commands,
    },
    Suite {
        id: 7,
        name: "lint-leakage classify (200 rules)",
        unit: "rules",
        setup: suite_lint_leakage,
    },
    Suite {
        id: 8,
        name: "git history scan (2,000 commits)",
        unit: "commits",
        setup: suite_history,
    },
    Suite {
        id: 9,
        name: "plan drift (300 Spec Kit tasks)",
        unit: "tasks",
        setup: suite_plan_drift,
    },
    Suite {
        id: 10,
        name: "promotion plan (40 rules)",
        unit: "rules",
        setup: suite_promotion,
    },
    Suite {
        id: 11,
        name: "SARIF render (1,000 findings)",
        unit: "findings",
        setup: suite_sarif,
    },
    Suite {
        id: 12,
        name: "forget pipeline (monorepo)",
        unit: "plans",
        setup: suite_forget,
    },
];

fn fmt_duration(ns: u128) -> String {
    let ns = ns as f64;
    if ns >= 1e9 {
        format!("{:.2} s", ns / 1e9)
    } else if ns >= 1e6 {
        format!("{:.2} ms", ns / 1e6)
    } else if ns >= 1e3 {
        format!("{:.2} µs", ns / 1e3)
    } else {
        format!("{ns:.0} ns")
    }
}

fn fmt_rate(r: f64, unit: &str) -> String {
    if r >= 1e6 {
        format!("{:.2}M {unit}/s", r / 1e6)
    } else if r >= 1e3 {
        format!("{:.1}k {unit}/s", r / 1e3)
    } else {
        format!("{r:.1} {unit}/s")
    }
}

fn run_suite(s: &Suite, dir: &Path, budget: Duration, max_iters: u64) -> Result<Result1> {
    let (mut f, units) = (s.setup)(dir)?;
    f(); // warm-up
    let mut times = Vec::new();
    let start = Instant::now();
    while (start.elapsed() < budget || times.len() < 3) && (times.len() as u64) < max_iters {
        let t = Instant::now();
        f();
        times.push(t.elapsed().as_nanos());
    }
    let n = times.len() as u128;
    let avg = times.iter().sum::<u128>() / n.max(1);
    Ok(Result1 {
        id: s.id,
        name: s.name.into(),
        iterations: n as u64,
        avg_ns: avg,
        min_ns: times.iter().copied().min().unwrap_or(0),
        max_ns: times.iter().copied().max().unwrap_or(0),
        throughput: if avg > 0 {
            units * 1e9 / avg as f64
        } else {
            0.0
        },
        unit: s.unit.into(),
    })
}

fn main() -> Result<()> {
    let mut filter: Option<String> = None;
    let mut json = false;
    let mut quick = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--filter" => {
                filter = Some(
                    args.next()
                        .context("--filter needs a value")?
                        .to_lowercase(),
                )
            }
            "--json" => json = true,
            "--quick" => quick = true,
            "--list" => {
                for s in SUITES {
                    println!("{:>2}  {}", s.id, s.name);
                }
                return Ok(());
            }
            "-h" | "--help" => {
                println!("mnemoa-bench [--filter TEXT] [--json] [--quick] [--list]");
                return Ok(());
            }
            other => anyhow::bail!("unknown argument `{other}`"),
        }
    }
    let (budget, max_iters) = if quick {
        (Duration::from_millis(200), 20)
    } else {
        (Duration::from_secs(2), 10_000)
    };
    let dir = tempfile::tempdir()?;
    let hw = hardware();
    let mut results = Vec::new();
    let mut skipped = Vec::new();
    for s in SUITES {
        if let Some(f) = &filter
            && !s.name.to_lowercase().contains(f)
            && s.id.to_string() != *f
        {
            continue;
        }
        let suite_dir: PathBuf = dir.path().join(format!("s{}", s.id));
        std::fs::create_dir_all(&suite_dir)?;
        match run_suite(s, &suite_dir, budget, max_iters) {
            Ok(r) => {
                if !json {
                    eprintln!("  done: {}", s.name);
                }
                results.push(r)
            }
            Err(e) => skipped.push(format!("{}: {e}", s.name)),
        }
    }
    let out = Output {
        version: mnemoa_core::VERSION.into(),
        hardware: hw,
        results,
        skipped,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!(
        "Hardware: {} ({} cores), {}, {} build",
        out.hardware.cpu, out.hardware.cores, out.hardware.os, out.hardware.profile
    );
    println!();
    println!("| # | Suite | Iterations | Avg latency | Throughput |");
    println!("|---|---|---:|---:|---:|");
    for r in &out.results {
        println!(
            "| {} | {} | {} | {} | {} |",
            r.id,
            r.name,
            r.iterations,
            fmt_duration(r.avg_ns),
            fmt_rate(r.throughput, &r.unit)
        );
    }
    for s in &out.skipped {
        println!("\nskipped: {s}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_suite_runs_once() {
        let dir = tempfile::tempdir().unwrap();
        for s in SUITES {
            let d = dir.path().join(format!("s{}", s.id));
            std::fs::create_dir_all(&d).unwrap();
            if s.id == 8 && !git_available() {
                continue;
            }
            let r = run_suite(s, &d, Duration::from_millis(1), 3)
                .unwrap_or_else(|e| panic!("{}: {e}", s.name));
            assert!(r.iterations >= 1 && r.avg_ns > 0, "{}", s.name);
        }
        assert_eq!(SUITES.len(), 12);
    }

    #[test]
    fn big_doc_is_about_1000_lines() {
        let n = big_doc().lines().count();
        assert!((1000..1015).contains(&n), "{n}");
    }

    #[test]
    fn formatting() {
        assert_eq!(fmt_duration(1_500), "1.50 µs");
        assert_eq!(fmt_duration(2_500_000), "2.50 ms");
        assert_eq!(fmt_rate(2_500.0, "files"), "2.5k files/s");
    }
}
