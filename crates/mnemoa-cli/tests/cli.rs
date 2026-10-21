use assert_cmd::Command;
use predicates::prelude::*;

fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (p, c) in files {
        let path = dir.path().join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, c).unwrap();
    }
    dir
}

fn mnemoa() -> Command {
    Command::cargo_bin("mnemoa").unwrap()
}

#[test]
fn clean_repo_exits_zero() {
    let dir = repo(&[
        (
            "AGENTS.md",
            "- Read [the guide](docs/guide.md) before changing the API.\n",
        ),
        ("docs/guide.md", "x"),
    ]);
    mnemoa()
        .args(["scan", "--no-history", "--color", "never"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("0 errors, 0 warnings, 0 infos"));
}

#[test]
fn broken_link_exits_one_with_json() {
    let dir = repo(&[("AGENTS.md", "Read [x](missing.md) before release.\n")]);
    let out = mnemoa()
        .args(["scan", "--no-history", "--format", "json"])
        .arg(dir.path())
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["summary"]["errors"], 1);
    assert_eq!(v["findings"][0]["rule"], "MNA007");
}

#[test]
fn sarif_and_github_formats() {
    let dir = repo(&[("AGENTS.md", "Read [x](missing.md) before release.\n")]);
    let out = mnemoa()
        .args(["scan", "--no-history", "-f", "sarif"])
        .arg(dir.path())
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["version"], "2.1.0");
    mnemoa()
        .args(["scan", "--no-history", "-f", "github"])
        .arg(dir.path())
        .assert()
        .code(1)
        .stdout(predicate::str::starts_with("::error file=AGENTS.md,line=1"));
}

#[test]
fn write_baseline_then_scan_is_clean() {
    let dir = repo(&[("AGENTS.md", "Read [x](missing.md) before release.\n")]);
    mnemoa()
        .args(["scan", "--no-history", "--write-baseline"])
        .arg(dir.path())
        .assert()
        .success();
    assert!(dir.path().join(".mnemoa/baseline.json").is_file());
    mnemoa()
        .args(["scan", "--no-history", "--color", "never"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("1 hidden by baseline"));
}

#[test]
fn strict_fails_on_warning() {
    let dir = repo(&[("AGENTS.md", "- Run `make lint`.\n")]);
    mnemoa()
        .args(["scan", "--no-history"])
        .arg(dir.path())
        .assert()
        .success();
    mnemoa()
        .args(["scan", "--no-history", "--strict"])
        .arg(dir.path())
        .assert()
        .code(1);
}

#[test]
fn rules_and_explain() {
    mnemoa()
        .arg("rules")
        .assert()
        .success()
        .stdout(predicate::str::contains("MNA013"));
    mnemoa()
        .args(["explain", "mna006"])
        .assert()
        .success()
        .stdout(predicate::str::contains("blind-reference"));
    mnemoa()
        .args(["explain", "MNA999"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown rule"));
}

#[test]
fn bad_target_is_a_usage_error() {
    let dir = repo(&[("AGENTS.md", "x\n")]);
    mnemoa()
        .args(["scan", "--targets", "vim"])
        .arg(dir.path())
        .assert()
        .code(2);
}

#[test]
fn budget_lists_targets() {
    let dir = repo(&[
        ("CLAUDE.md", "@AGENTS.md\n"),
        ("AGENTS.md", "- Use pnpm.\n"),
    ]);
    mnemoa()
        .args([
            "budget",
            "--color",
            "never",
            "--targets",
            "claude-code,codex",
        ])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(
            predicate::str::contains("claude-code")
                .and(predicate::str::contains("imported by CLAUDE.md")),
        );
    let out = mnemoa()
        .args(["budget", "-f", "json"])
        .arg(dir.path())
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["tokenizer"].as_str().unwrap().contains("estimate"));
}

#[test]
fn budget_check_fails_over_budget() {
    let dir = repo(&[
        ("AGENTS.md", "- Use pnpm.\n"),
        (".mnemoa.toml", "[rules]\ntoken_budget = 1\n"),
    ]);
    mnemoa()
        .args(["budget", "--check"])
        .arg(dir.path())
        .assert()
        .code(1);
}

#[test]
fn drift_reports_missing_symbol() {
    let dir = repo(&[
        ("AGENTS.md", "- Call `load_config()` at startup.\n"),
        ("src/lib.rs", "pub fn other() {}\n"),
    ]);
    mnemoa()
        .args(["drift", "--color", "never"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("missing-symbol"));
    mnemoa()
        .args(["drift", "--strict"])
        .arg(dir.path())
        .assert()
        .code(1);
}

#[test]
fn drift_base_needs_git() {
    let dir = repo(&[("AGENTS.md", "x\n")]);
    if std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["rev-parse", "--git-dir"])
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        mnemoa()
            .args(["drift", "--base", "main"])
            .arg(dir.path())
            .assert()
            .code(2);
    }
}
