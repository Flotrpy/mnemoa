//! `--server`: run commands against a mnemoa-api instance.

use anyhow::{Context as _, Result, bail};
use mnemoa_core::report::{self, Format};
use serde_json::{Value, json};

use crate::{Cli, Command, print, use_color};

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .context("building HTTP client")
}

fn send(req: reqwest::blocking::RequestBuilder) -> Result<Value> {
    let resp = req.send().context("contacting mnemoa-api")?;
    let status = resp.status();
    let v: Value = resp.json().context("reading mnemoa-api response")?;
    if !status.is_success() {
        bail!(
            "mnemoa-api returned {status}: {}",
            v["error"].as_str().unwrap_or("unknown error")
        );
    }
    Ok(v)
}

fn post(url: &str, endpoint: &str, body: Value) -> Result<Value> {
    send(
        client()?
            .post(format!("{}/{endpoint}", url.trim_end_matches('/')))
            .json(&body),
    )
}

fn get(url: &str, endpoint: &str) -> Result<Value> {
    send(client()?.get(format!("{}/{endpoint}", url.trim_end_matches('/'))))
}

fn path_str(p: &std::path::Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Run `cli.command` on the server at `url`.
pub fn run(url: &str, cli: &Cli) -> Result<i32> {
    let color = use_color(cli.global.color);
    let targets = cli.global.targets.clone();
    match &cli.command {
        Command::Scan(a) => {
            if a.write_baseline || a.baseline.is_some() {
                bail!("--baseline and --write-baseline are not supported with --server");
            }
            let format: Format = a.format.parse().map_err(anyhow::Error::msg)?;
            let v = post(
                url,
                "scan",
                json!({"path": path_str(&a.path), "targets": targets, "rules": a.rules, "strict": a.strict.then_some(true)}),
            )?;
            let r: mnemoa_core::ScanReport = serde_json::from_value(v)?;
            print(&report::render(&r, format, color))?;
            Ok(r.exit_code())
        }
        Command::Budget(a) => {
            let format: Format = a.format.parse().map_err(anyhow::Error::msg)?;
            let v = post(
                url,
                "budget",
                json!({"path": path_str(&a.path), "targets": targets}),
            )?;
            let r: mnemoa_core::budget::BudgetReport = serde_json::from_value(v)?;
            print(&report::render_budget(&r, format, color))?;
            Ok(0)
        }
        Command::Drift(a) => {
            let format: Format = a.format.parse().map_err(anyhow::Error::msg)?;
            let v = post(
                url,
                "drift",
                json!({"path": path_str(&a.path), "base": a.base}),
            )?;
            let r: mnemoa_core::drift::DriftReport = serde_json::from_value(v)?;
            print(&report::render_drift(&r, format, color))?;
            Ok(r.exit_code(a.strict))
        }
        Command::Promote(a) => {
            let v = post(
                url,
                "promote",
                json!({"path": path_str(&a.path), "llm": a.llm, "apply": a.apply}),
            )?;
            print(&(serde_json::to_string_pretty(&v)? + "\n"))?;
            Ok(0)
        }
        Command::Forget(a) => {
            if a.apply && !a.yes {
                bail!("--apply needs --yes with --server");
            }
            let v = post(
                url,
                "forget",
                json!({"path": path_str(&a.path), "apply": a.apply}),
            )?;
            print(&(serde_json::to_string_pretty(&v)? + "\n"))?;
            Ok(0)
        }
        Command::Rules { .. } => {
            print(&(serde_json::to_string_pretty(&get(url, "rules")?)? + "\n"))?;
            Ok(0)
        }
        Command::Explain { id } => {
            print(&(serde_json::to_string_pretty(&get(url, &format!("rules/{id}"))?)? + "\n"))?;
            Ok(0)
        }
        Command::Retire(_) | Command::Init { .. } => {
            bail!("this command runs locally only; drop --server")
        }
    }
}
