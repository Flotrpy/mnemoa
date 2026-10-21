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

#[test]
fn promote_dry_run_then_apply() {
    let dir = repo(&[(
        "AGENTS.md",
        "- Use 2 spaces for indentation.\n- Keep PRs small.\n",
    )]);
    mnemoa()
        .args(["promote", "--color", "never"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(
            predicate::str::contains("+++ b/.editorconfig")
                .and(predicate::str::contains("would change")),
        );
    assert!(!dir.path().join(".editorconfig").exists());
    mnemoa()
        .args(["promote", "--apply"])
        .arg(dir.path())
        .assert()
        .success();
    assert!(
        std::fs::read_to_string(dir.path().join(".editorconfig"))
            .unwrap()
            .contains("indent_size = 2")
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap(),
        "- Keep PRs small.\n"
    );
    assert!(dir.path().join(".mnemoa/ledger.toml").is_file());
}

#[test]
fn promote_llm_without_feature_or_config_fails() {
    let dir = repo(&[("AGENTS.md", "- Never edit `gen/x.rs` by hand.\n")]);
    mnemoa()
        .args(["promote", "--llm"])
        .arg(dir.path())
        .assert()
        .code(2);
}

#[test]
fn retire_dry_run_then_apply() {
    let dir = repo(&[(
        "AGENTS.md",
        "<!-- why: freeze | retire-when: date:2020-01-01 -->\n- Keep PRs tiny.\n<!-- why: outage | retire-when: date:2020-01-01 -->\n- Never touch prod data.\n",
    )]);
    let out = mnemoa()
        .args(["retire", "-f", "json"])
        .arg(dir.path())
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["candidates"].as_array().unwrap().len(), 2);
    mnemoa()
        .args(["retire", "--apply"])
        .arg(dir.path())
        .assert()
        .success();
    let after = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert!(!after.contains("PRs tiny") && after.contains("prod data"));
}

#[test]
fn forget_dry_run_and_apply() {
    let dir = repo(&[(
        "AGENTS.md",
        "# P\n\n- Use 4 spaces for indentation.\n\n## Releasing\n\n1. Bump the version in Cargo.toml and update the changelog with every user-facing change.\n2. Tag the release commit with the new version and push the tag to origin.\n3. Wait for the release workflow, then check the published artifacts and the docs site.\n",
    )]);
    mnemoa()
        .args(["forget", "--color", "never", "--targets", "claude-code"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(
            predicate::str::contains("move")
                .and(predicate::str::contains("claude-code"))
                .and(predicate::str::contains("+++ b/docs/agents/releasing.md")),
        );
    mnemoa()
        .args(["forget", "--apply"])
        .arg(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--yes"));
    mnemoa()
        .args(["forget", "--apply", "--yes"])
        .arg(dir.path())
        .assert()
        .success();
    assert!(dir.path().join("docs/agents/releasing.md").is_file());
    assert!(dir.path().join(".editorconfig").is_file());
    let out = mnemoa()
        .args(["forget", "-f", "json"])
        .arg(dir.path())
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["actions"].as_array().unwrap().len(), 0);
}

#[test]
fn init_creates_scaffold_and_never_overwrites() {
    let dir = repo(&[("Makefile", "test:\n\tcargo test\n")]);
    mnemoa()
        .arg("init")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("created  AGENTS.md"));
    let agents = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert!(agents.contains("`make test`"));
    mnemoa()
        .args(["scan", "--no-history"])
        .arg(dir.path())
        .assert()
        .success();
    std::fs::write(dir.path().join("AGENTS.md"), "mine\n").unwrap();
    mnemoa()
        .args(["init", "--ci"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("skipped  AGENTS.md"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap(),
        "mine\n"
    );
    assert!(dir.path().join(".github/workflows/mnemoa.yml").is_file());
}
