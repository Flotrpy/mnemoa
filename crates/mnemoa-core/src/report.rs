//! Render scan reports as pretty text, JSON, SARIF 2.1.0 or GitHub workflow annotations.

use std::fmt::Write as _;
use std::str::FromStr;

use serde_json::{Value, json};

use crate::diag::{Finding, Severity};
use crate::rules::{self, RuleInfo};
use crate::scan::ScanReport;

/// Output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Pretty,
    Json,
    Sarif,
    Github,
}

impl FromStr for Format {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "pretty" | "text" => Ok(Format::Pretty),
            "json" => Ok(Format::Json),
            "sarif" => Ok(Format::Sarif),
            "github" | "gha" => Ok(Format::Github),
            other => Err(format!(
                "unknown format `{other}` (pretty, json, sarif, github)"
            )),
        }
    }
}

/// Render `report` in `format`. `color` only affects pretty output.
pub fn render(report: &ScanReport, format: Format, color: bool) -> String {
    match format {
        Format::Pretty => pretty(report, color),
        Format::Json => serde_json::to_string_pretty(report).expect("report serializes") + "\n",
        Format::Sarif => {
            serde_json::to_string_pretty(&sarif(report)).expect("sarif serializes") + "\n"
        }
        Format::Github => github(report),
    }
}

fn paint(s: &str, code: &str, color: bool) -> String {
    if color {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn sev_label(s: Severity, color: bool) -> String {
    match s {
        Severity::Error => paint("error", "31;1", color),
        Severity::Warn => paint("warn ", "33;1", color),
        Severity::Info => paint("info ", "36", color),
    }
}

/// Human-readable report grouped by file.
pub fn pretty(report: &ScanReport, color: bool) -> String {
    let mut out = String::new();
    let mut current = "";
    for f in &report.findings {
        if f.path != current {
            if !current.is_empty() {
                out.push('\n');
            }
            current = &f.path;
            let _ = writeln!(out, "{}", paint(&f.path, "1;4", color));
        }
        let _ = writeln!(
            out,
            "  {:>4}:{:<3} {} {} {}  {}",
            f.line,
            f.column,
            sev_label(f.severity, color),
            paint(&f.rule, "2", color),
            paint(&f.name, "2", color),
            f.message
        );
        if let Some(h) = &f.help {
            let _ = writeln!(out, "             {} {h}", paint("help:", "32", color));
        }
    }
    if !report.findings.is_empty() {
        out.push('\n');
    }
    let s = &report.summary;
    let plural = |n: usize, w: &str| {
        if n == 1 {
            format!("{n} {w}")
        } else {
            format!("{n} {w}s")
        }
    };
    let _ = write!(
        out,
        "{}, {}, {} in {} ({})",
        plural(s.errors, "error"),
        plural(s.warnings, "warning"),
        plural(s.infos, "info"),
        plural(report.files.len(), "file"),
        report
            .targets
            .iter()
            .map(|t| t.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    if report.baselined > 0 {
        let _ = write!(out, "; {} hidden by baseline", report.baselined);
    }
    out.push('\n');
    out
}

fn sarif_level(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warn => "warning",
        Severity::Info => "note",
    }
}

fn sarif_rule(r: &RuleInfo) -> Value {
    json!({
        "id": r.id,
        "name": r.name,
        "shortDescription": { "text": r.summary },
        "fullDescription": { "text": r.explain },
        "help": { "text": r.rationale },
        "helpUri": r.link,
        "defaultConfiguration": { "level": sarif_level(r.default_severity) },
        "properties": { "tags": ["agent-context"], "fixable": r.fixable }
    })
}

fn sarif_result(f: &Finding) -> Value {
    let rule_index = rules::RULES
        .iter()
        .position(|r| r.id == f.rule)
        .unwrap_or(0);
    let mut message = f.message.clone();
    if let Some(h) = &f.help {
        message.push(' ');
        message.push_str(h);
    }
    json!({
        "ruleId": f.rule,
        "ruleIndex": rule_index,
        "level": sarif_level(f.severity),
        "message": { "text": message },
        "locations": [{
            "physicalLocation": {
                "artifactLocation": { "uri": f.path, "uriBaseId": "%SRCROOT%" },
                "region": {
                    "startLine": f.line,
                    "startColumn": f.column,
                    "endLine": f.end_line,
                    "endColumn": f.end_column.max(1)
                }
            }
        }],
        "partialFingerprints": { "mnemoa/v1": f.fingerprint }
    })
}

/// SARIF 2.1.0 log suitable for GitHub code scanning.
pub fn sarif(report: &ScanReport) -> Value {
    json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "mnemoa",
                    "version": report.version,
                    "semanticVersion": report.version,
                    "informationUri": "https://github.com/Flotrpy/mnemoa",
                    "rules": rules::RULES.iter().map(sarif_rule).collect::<Vec<_>>()
                }
            },
            "originalUriBaseIds": {
                "%SRCROOT%": { "uri": "file:///" }
            },
            "results": report.findings.iter().map(sarif_result).collect::<Vec<_>>()
        }]
    })
}

fn escape_data(s: &str) -> String {
    s.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

fn escape_prop(s: &str) -> String {
    escape_data(s).replace(':', "%3A").replace(',', "%2C")
}

/// GitHub Actions workflow commands, one per finding.
pub fn github(report: &ScanReport) -> String {
    let mut out = String::new();
    for f in &report.findings {
        let kind = match f.severity {
            Severity::Error => "error",
            Severity::Warn => "warning",
            Severity::Info => "notice",
        };
        let mut msg = f.message.clone();
        if let Some(h) = &f.help {
            msg.push('\n');
            msg.push_str(h);
        }
        let _ = writeln!(
            out,
            "::{kind} file={},line={},col={},endLine={},endColumn={},title={}::{}",
            escape_prop(&f.path),
            f.line,
            f.column,
            f.end_line,
            f.end_column,
            escape_prop(&format!("{} {}", f.rule, f.name)),
            escape_data(&msg)
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Summary;
    use crate::discover::Target;

    fn report() -> ScanReport {
        let doc = crate::doc::Doc::parse("AGENTS.md", "# T\n\n- [x](gone.md)\n");
        let f = Finding::at(
            "MNA007",
            "broken-reference",
            Severity::Error,
            &doc,
            5..18,
            "Link target `gone.md` does not exist.",
        )
        .with_help("Fix the path, or remove: it.");
        ScanReport {
            version: "0.1.0".into(),
            root: ".".into(),
            targets: vec![Target::ClaudeCode],
            config_source: "defaults".into(),
            strict: false,
            summary: Summary::of(std::slice::from_ref(&f)),
            findings: vec![f],
            baselined: 0,
            files: vec!["AGENTS.md".into()],
        }
    }

    #[test]
    fn pretty_groups_by_file() {
        let s = pretty(&report(), false);
        assert!(s.starts_with("AGENTS.md\n"));
        assert!(s.contains("3:1"));
        assert!(s.contains("MNA007"));
        assert!(s.contains("1 error, 0 warnings, 0 infos in 1 file (claude-code)"));
        assert!(!s.contains('\x1b'));
        assert!(pretty(&report(), true).contains('\x1b'));
    }

    #[test]
    fn json_round_trips() {
        let s = render(&report(), Format::Json, false);
        let back: ScanReport = serde_json::from_str(&s).unwrap();
        assert_eq!(back.findings.len(), 1);
    }

    #[test]
    fn sarif_has_required_shape() {
        let v = sarif(&report());
        assert_eq!(v["version"], "2.1.0");
        let run = &v["runs"][0];
        assert_eq!(run["tool"]["driver"]["name"], "mnemoa");
        assert_eq!(
            run["tool"]["driver"]["rules"].as_array().unwrap().len(),
            rules::RULES.len()
        );
        let r = &run["results"][0];
        assert_eq!(r["ruleId"], "MNA007");
        assert_eq!(r["ruleIndex"], 6);
        assert_eq!(r["level"], "error");
        assert_eq!(
            r["locations"][0]["physicalLocation"]["region"]["startLine"],
            3
        );
    }

    #[test]
    fn github_annotations_escape() {
        let s = github(&report());
        assert!(s.starts_with("::error file=AGENTS.md,line=3,col=1,"));
        assert!(s.contains("title=MNA007 broken-reference::"));
        assert!(s.contains("%0AFix the path, or remove: it."));
        assert_eq!(s.lines().count(), 1);
    }

    #[test]
    fn format_parsing() {
        assert_eq!("SARIF".parse::<Format>().unwrap(), Format::Sarif);
        assert!("xml".parse::<Format>().is_err());
    }
}

/// Render a drift report as pretty text, JSON or GitHub annotations.
pub fn render_drift(report: &crate::drift::DriftReport, format: Format, color: bool) -> String {
    match format {
        Format::Json | Format::Sarif => {
            serde_json::to_string_pretty(report).expect("drift serializes") + "\n"
        }
        Format::Github => {
            let mut out = String::new();
            for i in &report.items {
                let kind = match i.severity {
                    Severity::Error => "error",
                    Severity::Warn => "warning",
                    Severity::Info => "notice",
                };
                let _ = writeln!(
                    out,
                    "::{kind} file={},line={},col={},title={}::{}",
                    escape_prop(&i.path),
                    i.line,
                    i.column,
                    escape_prop(&format!(
                        "drift {}",
                        serde_json::to_value(i.kind)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_string))
                            .unwrap_or_default()
                    )),
                    escape_data(&i.message)
                );
            }
            out
        }
        Format::Pretty => {
            let mut out = String::new();
            let mut current = "";
            for i in &report.items {
                if i.path != current {
                    if !current.is_empty() {
                        out.push('\n');
                    }
                    current = &i.path;
                    let _ = writeln!(out, "{}", paint(&i.path, "1;4", color));
                }
                let kind = serde_json::to_value(i.kind)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                let _ = writeln!(
                    out,
                    "  {:>4}:{:<3} {} {}  {}",
                    i.line,
                    i.column,
                    sev_label(i.severity, color),
                    paint(&kind, "2", color),
                    i.message
                );
            }
            let stale: Vec<&crate::drift::Freshness> = report
                .freshness
                .iter()
                .filter(|f| f.commits_since > 0)
                .collect();
            if !stale.is_empty() {
                if !report.items.is_empty() {
                    out.push('\n');
                }
                let _ = writeln!(
                    out,
                    "{}",
                    paint(
                        "Freshness (commits touching referenced paths since the doc last changed)",
                        "1",
                        color
                    )
                );
                for f in stale {
                    let _ = writeln!(out, "  {:>4}  {}", f.commits_since, f.path);
                }
            }
            if !out.is_empty() {
                out.push('\n');
            }
            let s = &report.summary;
            let _ = write!(
                out,
                "drift: {} error(s), {} warning(s), {} info",
                s.errors, s.warnings, s.infos
            );
            if let Some(b) = &report.base {
                let _ = write!(out, " against {b}");
            }
            if !report.history {
                out.push_str(" (no git history: freshness and plan checks limited)");
            }
            out.push('\n');
            out
        }
    }
}

/// Render a budget report as a pretty table or JSON.
pub fn render_budget(report: &crate::budget::BudgetReport, format: Format, color: bool) -> String {
    if matches!(format, Format::Json | Format::Sarif | Format::Github) {
        return serde_json::to_string_pretty(report).expect("budget serializes") + "\n";
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Always-loaded context per target ({}).\n",
        report.tokenizer
    );
    for t in &report.targets {
        let _ = writeln!(
            out,
            "{}  {} tokens, {} lines, {} bytes",
            paint(t.target.as_str(), "1", color),
            t.total_tokens,
            t.total_lines,
            t.total_bytes
        );
        if t.always.is_empty() {
            let _ = writeln!(out, "    (nothing loaded at start)");
        }
        for c in &t.always {
            let via = c
                .via
                .as_ref()
                .map(|v| format!("  (imported by {v})"))
                .unwrap_or_default();
            let _ = writeln!(out, "    {:>6}  {}{via}", c.tokens, c.path);
        }
        if !t.on_demand.is_empty() {
            let _ = writeln!(out, "    on demand:");
            for o in &t.on_demand {
                let _ = writeln!(out, "    {:>6}  {}  — {}", o.tokens, o.path, o.trigger);
            }
        }
        if let Some(d) = t.dropped_bytes {
            let _ = writeln!(
                out,
                "    {} {d} bytes are past the cap and never load",
                paint("!", "33;1", color)
            );
        }
        for n in &t.notes {
            let _ = writeln!(out, "    note: {n}");
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod more_tests {
    use super::*;
    use crate::config::Config;
    use crate::discover::Target;
    use crate::drift::{DriftOptions, drift};
    use crate::tokens::Tokenizer;
    use crate::workspace::tests::ws;

    #[test]
    fn drift_and_budget_render() {
        let w = ws(&[
            ("AGENTS.md", "- See [x](gone.md) when stuck.\n"),
            ("CLAUDE.md", "@AGENTS.md\n"),
        ]);
        let d = drift(&w, &Config::default(), None, &DriftOptions::default()).unwrap();
        let p = render_drift(&d, Format::Pretty, false);
        assert!(p.contains("missing-path") && p.contains("drift: 1 error(s)"));
        assert!(render_drift(&d, Format::Github, false).starts_with("::error file=AGENTS.md"));
        let b = crate::budget::budget(&w, &[Target::ClaudeCode], Tokenizer::Heuristic);
        let s = render_budget(&b, Format::Pretty, false);
        assert!(s.contains("claude-code") && s.contains("(imported by CLAUDE.md)"));
        let j: serde_json::Value =
            serde_json::from_str(&render_budget(&b, Format::Json, false)).unwrap();
        assert_eq!(j["targets"][0]["target"], "claude-code");
    }
}
