//! Snapshot tests over the fixture corpus in `fixtures/`.
//!
//! Update snapshots with `INSTA_UPDATE=always cargo test -p mnemoa-core --test fixtures`
//! (or `cargo insta review`) and review the diff.

use std::path::PathBuf;

use mnemoa_core::config::Config;
use mnemoa_core::drift::{DriftOptions, drift};
use mnemoa_core::report::{self, Format};
use mnemoa_core::scan::{ScanOptions, scan, scan_workspace};
use mnemoa_core::{Target, Tokenizer, Workspace};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn config(name: &str) -> Config {
    Config::load_file(&fixture(name), None).unwrap()
}

/// Scan without git history so results do not depend on this repo's commits.
fn scan_pretty(name: &str, targets: &[Target]) -> String {
    let opts = ScanOptions {
        targets: Some(targets.to_vec()),
        history: false,
        ..Default::default()
    };
    let r = scan(&fixture(name), &config(name), &opts).unwrap();
    report::render(&r, Format::Pretty, false)
}

const ALL: [Target; 7] = Target::ALL;

#[test]
fn clean_fixture_has_no_findings() {
    insta::assert_snapshot!(scan_pretty("clean", &ALL));
}

#[test]
fn smelly_fixture_findings() {
    insta::assert_snapshot!(scan_pretty("smelly", &Target::DEFAULT));
}

#[test]
fn smelly_fixture_triggers_every_rule_without_history() {
    let opts = ScanOptions {
        history: false,
        ..Default::default()
    };
    let r = scan(&fixture("smelly"), &config("smelly"), &opts).unwrap();
    let rules: std::collections::BTreeSet<&str> =
        r.findings.iter().map(|f| f.rule.as_str()).collect();
    for id in [
        "MNA001", "MNA002", "MNA003", "MNA004", "MNA006", "MNA007", "MNA008", "MNA009", "MNA010",
        "MNA011", "MNA012", "MNA013",
    ] {
        assert!(rules.contains(id), "{id} did not fire on fixtures/smelly");
    }
}

#[test]
fn kiro_and_cline_and_monorepo_fixtures() {
    insta::assert_snapshot!("kiro", scan_pretty("kiro", &ALL));
    insta::assert_snapshot!("cline", scan_pretty("cline", &ALL));
    insta::assert_snapshot!("monorepo", scan_pretty("monorepo", &ALL));
}

#[test]
fn budget_per_target() {
    let ws = Workspace::load(&fixture("monorepo"), &Default::default()).unwrap();
    let b = mnemoa_core::budget::budget(&ws, &ALL, Tokenizer::Heuristic);
    insta::assert_snapshot!(report::render_budget(&b, Format::Pretty, false));
}

#[test]
fn speckit_plan_drift() {
    let ws = Workspace::load(&fixture("speckit"), &Default::default()).unwrap();
    let mut d = drift(&ws, &Config::default(), None, &DriftOptions::default()).unwrap();
    d.root = "<root>".into();
    insta::assert_json_snapshot!(d.items);
}

#[test]
fn kiro_task_drift() {
    let ws = Workspace::load(&fixture("kiro"), &Default::default()).unwrap();
    let d = drift(&ws, &Config::default(), None, &DriftOptions::default()).unwrap();
    insta::assert_json_snapshot!(d.items);
}

#[test]
fn smelly_forget_plan() {
    let ws = Workspace::load(&fixture("smelly"), &config("smelly").discover_options()).unwrap();
    let p = mnemoa_core::forget::plan(&ws, &config("smelly"), "2026-10-21", None);
    let today = mnemoa_core::ledger::today();
    let diff = p.diff.replace(&today, "<today>");
    let actions: Vec<String> = p
        .actions
        .iter()
        .map(|a| format!("{}:{} {:?} {}", a.path, a.line, a.kind, a.summary))
        .collect();
    insta::assert_snapshot!("smelly_forget_actions", actions.join("\n"));
    insta::assert_snapshot!("smelly_forget_diff", diff);
    // AGENTS.md shrinks for agents that read it directly...
    let codex = p.tokens.iter().find(|t| t.target == Target::Codex).unwrap();
    assert!(codex.after < codex.before, "{codex:?}");
    // ...while Claude Code, which only saw the 2-line CLAUDE.md before, now imports AGENTS.md.
    let tokens: Vec<String> = p
        .tokens
        .iter()
        .map(|t| format!("{}: {} -> {}", t.target, t.before, t.after))
        .collect();
    insta::assert_snapshot!("smelly_forget_tokens", tokens.join("\n"));
}

#[test]
fn sarif_output_validates_against_the_official_schema() {
    let schema: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture("schemas/sarif-2.1.0.json")).unwrap(),
    )
    .unwrap();
    let validator = jsonschema::draft4::new(&schema).unwrap();
    let ws = Workspace::load(&fixture("smelly"), &Default::default()).unwrap();
    let cfg = config("smelly");
    let r = scan_workspace(&ws, &cfg, &ScanOptions::default(), None);
    assert!(!r.findings.is_empty());
    let sarif = report::sarif(&r);
    let errors: Vec<String> = validator
        .iter_errors(&sarif)
        .map(|e| format!("{e} at {}", e.instance_path()))
        .collect();
    assert!(errors.is_empty(), "SARIF schema errors: {errors:#?}");
}
