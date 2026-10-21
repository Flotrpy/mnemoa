//! `mnemoa --server`: the CLI against an in-process mnemoa-api must exit like the local CLI.

use std::path::Path;
use std::sync::Arc;

use assert_cmd::Command;
use mnemoa_api::{AppState, router};
use mnemoa_core::config::Config;

fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (p, c) in files {
        let path = dir.path().join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, c).unwrap();
    }
    dir
}

/// Serve the API for `root` on a free local port; returns its URL.
fn server(root: &Path) -> String {
    let mut config = Config::default();
    config.api.root = root.to_path_buf();
    let state = Arc::new(AppState::new(config).unwrap());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                let l = tokio::net::TcpListener::from_std(listener).unwrap();
                axum::serve(l, router(state)).await.unwrap();
            });
    });
    format!("http://{addr}")
}

fn remote(url: &str) -> Command {
    let mut c = Command::cargo_bin("mnemoa").unwrap();
    c.env("NO_PROXY", "127.0.0.1")
        .env("no_proxy", "127.0.0.1")
        .args(["--server", url]);
    c
}

#[test]
fn remote_budget_check_fails_over_budget() {
    let dir = repo(&[
        ("AGENTS.md", "- Use pnpm.\n"),
        (".mnemoa.toml", "[rules]\ntoken_budget = 1\n"),
    ]);
    let url = server(dir.path());
    remote(&url)
        .arg("budget")
        .arg(dir.path())
        .assert()
        .success();
    remote(&url)
        .args(["budget", "--check"])
        .arg(dir.path())
        .assert()
        .code(1);
}

#[test]
fn remote_budget_check_passes_under_budget() {
    let dir = repo(&[("AGENTS.md", "- Use pnpm.\n")]);
    let url = server(dir.path());
    remote(&url)
        .args(["budget", "--check"])
        .arg(dir.path())
        .assert()
        .success();
}

#[test]
fn remote_drift_follows_repo_strict_setting() {
    let files = [
        ("AGENTS.md", "- Call `load_config()` at startup.\n"),
        ("src/lib.rs", "pub fn other() {}\n"),
    ];
    let lenient = repo(&files);
    let url = server(lenient.path());
    remote(&url)
        .arg("drift")
        .arg(lenient.path())
        .assert()
        .success();
    remote(&url)
        .args(["drift", "--strict"])
        .arg(lenient.path())
        .assert()
        .code(1);

    let mut strict_files = files.to_vec();
    strict_files.push((".mnemoa.toml", "[scan]\nstrict = true\n"));
    let strict = repo(&strict_files);
    let url = server(strict.path());
    remote(&url)
        .arg("drift")
        .arg(strict.path())
        .assert()
        .code(1);
}

fn git(dir: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn rules(out: &std::process::Output) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["rule"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn remote_scan_honors_no_history() {
    let dir = repo(&[
        ("AGENTS.md", "- Keep PRs small.\n"),
        (".mnemoa.toml", "[rules]\nfossil_commits = 1\n"),
        ("src/a.rs", "fn a() {}\n"),
    ]);
    let p = dir.path();
    if !(git(p, &["init", "-q"]) && git(p, &["add", "."]) && git(p, &["commit", "-qm", "init"])) {
        eprintln!("git unavailable; skipping");
        return;
    }
    std::fs::write(p.join("src/a.rs"), "fn b() {}\n").unwrap();
    assert!(git(p, &["commit", "-qam", "code"]));
    let url = server(p);
    let with = remote(&url)
        .args(["scan", "-f", "json"])
        .arg(p)
        .output()
        .unwrap();
    assert!(rules(&with).contains(&"MNA005".to_string()), "{with:?}");
    let without = remote(&url)
        .args(["scan", "--no-history", "-f", "json"])
        .arg(p)
        .output()
        .unwrap();
    assert!(
        !rules(&without).contains(&"MNA005".to_string()),
        "{without:?}"
    );
}
