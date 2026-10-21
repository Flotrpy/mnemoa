//! mnemoa HTTP API for CI bots and editors.
//!
//! Every request names a `path` that must resolve inside `MNEMOA_ROOT` (after following
//! symlinks). Writes (`apply: true`) are refused unless `MNEMOA_API_ALLOW_WRITE=true`.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use mnemoa_core::config::Config;
use mnemoa_core::drift::DriftOptions;
use mnemoa_core::history::History;
use mnemoa_core::scan::{self, ScanOptions};
use mnemoa_core::{Target, Tokenizer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Server state shared by handlers.
pub struct AppState {
    /// Canonical root every request path must stay inside.
    pub root: PathBuf,
    pub config: Config,
    pub started: Instant,
    pub requests: AtomicU64,
    pub scans: AtomicU64,
    pub findings_by_rule: Mutex<BTreeMap<String, u64>>,
}

impl AppState {
    /// Build state from a config. The root is canonicalized here.
    pub fn new(config: Config) -> std::io::Result<Self> {
        let root = config.api.root.canonicalize()?;
        Ok(AppState {
            root,
            config,
            started: Instant::now(),
            requests: AtomicU64::new(0),
            scans: AtomicU64::new(0),
            findings_by_rule: Mutex::new(BTreeMap::new()),
        })
    }
}

/// An error response: `{"error": "..."}` with a status code.
#[derive(Debug)]
pub struct ApiError(StatusCode, String);

impl ApiError {
    fn bad(msg: impl Into<String>) -> Self {
        ApiError(StatusCode::BAD_REQUEST, msg.into())
    }
    fn forbidden(msg: impl Into<String>) -> Self {
        ApiError(StatusCode::FORBIDDEN, msg.into())
    }
    fn internal(msg: impl std::fmt::Display) -> Self {
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, msg.to_string())
    }
    /// A failed write: 403 when the core refused a path outside the repository.
    fn write(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            ApiError::forbidden(e.to_string())
        } else {
            ApiError::internal(e)
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

/// Resolve a request path inside the root. Rejects `..` components, absolute paths outside the
/// root, missing paths and symlinks that lead outside.
pub fn resolve(root: &Path, requested: &str) -> Result<PathBuf, ApiError> {
    let req = Path::new(requested.trim());
    if req.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(ApiError::forbidden("path must not contain `..`"));
    }
    let joined = if req.is_absolute() {
        req.to_path_buf()
    } else {
        root.join(req)
    };
    let canon = joined
        .canonicalize()
        .map_err(|_| ApiError::bad(format!("path `{requested}` does not exist")))?;
    if !canon.starts_with(root) {
        return Err(ApiError::forbidden(format!(
            "path `{requested}` is outside MNEMOA_ROOT"
        )));
    }
    if !canon.is_dir() {
        return Err(ApiError::bad(format!(
            "path `{requested}` is not a directory"
        )));
    }
    Ok(canon)
}

fn parse_targets(t: &Option<Vec<String>>) -> Result<Option<Vec<Target>>, ApiError> {
    match t {
        None => Ok(None),
        Some(list) => list
            .iter()
            .map(|s| s.parse::<Target>().map_err(ApiError::bad))
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
    }
}

/// Per-request config: the repo's `.mnemoa.toml` plus the server's environment overrides.
fn repo_config(state: &AppState, path: &Path) -> Result<Config, ApiError> {
    let mut c = Config::load(path, None).map_err(|e| ApiError::bad(e.to_string()))?;
    c.api = state.config.api.clone();
    c.llm = state.config.llm.clone();
    Ok(c)
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(ApiError::internal)?
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LlmStatus {
    pub enabled: bool,
    pub compiled: bool,
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub config_source: String,
    pub root: String,
    pub allow_write: bool,
    pub llm: LlmStatus,
}

async fn health(State(s): State<Arc<AppState>>) -> Json<HealthResponse> {
    s.requests.fetch_add(1, Ordering::Relaxed);
    Json(HealthResponse {
        status: "ok".into(),
        version: mnemoa_core::VERSION.into(),
        config_source: s.config.source.clone(),
        root: s.root.display().to_string(),
        allow_write: s.config.api.allow_write,
        llm: LlmStatus {
            enabled: s.config.llm.enabled,
            compiled: cfg!(feature = "llm"),
            provider: s.config.llm.provider.clone(),
            model: s.config.llm.model.clone(),
        },
    })
}

#[derive(Debug, Deserialize)]
pub struct ScanRequest {
    #[serde(default = "dot")]
    pub path: String,
    pub targets: Option<Vec<String>>,
    pub rules: Option<Vec<String>>,
    pub strict: Option<bool>,
    /// Use git history (MNA005). Defaults to true.
    pub history: Option<bool>,
}

fn dot() -> String {
    ".".into()
}

async fn scan_handler(
    State(s): State<Arc<AppState>>,
    Json(req): Json<ScanRequest>,
) -> ApiResult<scan::ScanReport> {
    s.requests.fetch_add(1, Ordering::Relaxed);
    let path = resolve(&s.root, &req.path)?;
    let mut config = repo_config(&s, &path)?;
    if let Some(t) = parse_targets(&req.targets)? {
        config.targets = t;
    }
    let state = s.clone();
    let report = blocking(move || {
        let opts = ScanOptions {
            targets: None,
            rules: req.rules,
            strict: req.strict,
            baseline: None,
            history: req.history.unwrap_or(true),
        };
        scan::scan(&path, &config, &opts).map_err(|e| ApiError::bad(e.to_string()))
    })
    .await?;
    state.scans.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut m) = state.findings_by_rule.lock() {
        for f in &report.findings {
            *m.entry(f.rule.clone()).or_insert(0) += 1;
        }
    }
    Ok(Json(report))
}

#[derive(Debug, Deserialize)]
pub struct BudgetRequest {
    #[serde(default = "dot")]
    pub path: String,
    pub targets: Option<Vec<String>>,
}

async fn budget_handler(
    State(s): State<Arc<AppState>>,
    Json(req): Json<BudgetRequest>,
) -> ApiResult<mnemoa_core::budget::BudgetReport> {
    s.requests.fetch_add(1, Ordering::Relaxed);
    let path = resolve(&s.root, &req.path)?;
    let mut config = repo_config(&s, &path)?;
    if let Some(t) = parse_targets(&req.targets)? {
        config.targets = t;
    }
    let report = blocking(move || {
        let ws = scan::load_workspace(&path, &config).map_err(|e| ApiError::bad(e.to_string()))?;
        Ok(
            mnemoa_core::budget::budget(&ws, &config.targets, Tokenizer::best())
                .with_token_budget(config.rules.token_budget),
        )
    })
    .await?;
    Ok(Json(report))
}

#[derive(Debug, Deserialize)]
pub struct DriftRequest {
    #[serde(default = "dot")]
    pub path: String,
    pub base: Option<String>,
}

async fn drift_handler(
    State(s): State<Arc<AppState>>,
    Json(req): Json<DriftRequest>,
) -> ApiResult<mnemoa_core::drift::DriftReport> {
    s.requests.fetch_add(1, Ordering::Relaxed);
    let path = resolve(&s.root, &req.path)?;
    let config = repo_config(&s, &path)?;
    let report = blocking(move || {
        let ws = scan::load_workspace(&path, &config).map_err(|e| ApiError::bad(e.to_string()))?;
        let history =
            scan::open_history(&path, &config).map_err(|e| ApiError::bad(e.to_string()))?;
        if req.base.is_some() && history.is_none() {
            return Err(ApiError::bad("`base` needs a git repository"));
        }
        mnemoa_core::drift::drift(
            &ws,
            &config,
            history.as_ref().map(|h| h as &dyn History),
            &DriftOptions { base: req.base },
        )
        .map_err(|e| ApiError::bad(e.to_string()))
    })
    .await?;
    Ok(Json(report))
}

#[derive(Debug, Deserialize)]
pub struct PromoteRequest {
    #[serde(default = "dot")]
    pub path: String,
    #[serde(default)]
    pub llm: bool,
    #[serde(default)]
    pub apply: bool,
}

fn check_write(s: &AppState, apply: bool) -> Result<(), ApiError> {
    if apply && !s.config.api.allow_write {
        return Err(ApiError::forbidden(
            "apply is disabled; set MNEMOA_API_ALLOW_WRITE=true to allow writes",
        ));
    }
    Ok(())
}

#[cfg(feature = "llm")]
fn drafter(config: &Config) -> Result<Box<dyn mnemoa_core::drafter::Drafter>, ApiError> {
    Ok(Box::new(
        mnemoa_core::llm::HttpDrafter::new(&config.llm)
            .map_err(|e| ApiError::bad(e.to_string()))?,
    ))
}

#[cfg(not(feature = "llm"))]
fn drafter(_config: &Config) -> Result<Box<dyn mnemoa_core::drafter::Drafter>, ApiError> {
    Err(ApiError::bad(
        "this server was built without the `llm` feature",
    ))
}

fn with_written(plan: impl Serialize, written: Vec<String>) -> Result<Value, ApiError> {
    let mut v = serde_json::to_value(plan).map_err(ApiError::internal)?;
    v["written"] = json!(written);
    Ok(v)
}

async fn promote_handler(
    State(s): State<Arc<AppState>>,
    Json(req): Json<PromoteRequest>,
) -> ApiResult<Value> {
    s.requests.fetch_add(1, Ordering::Relaxed);
    check_write(&s, req.apply)?;
    let path = resolve(&s.root, &req.path)?;
    let config = repo_config(&s, &path)?;
    let v = blocking(move || {
        let ws = scan::load_workspace(&path, &config).map_err(|e| ApiError::bad(e.to_string()))?;
        let ast_grep = mnemoa_core::drafter::find_ast_grep();
        let d = if req.llm {
            Some(drafter(&config)?)
        } else {
            None
        };
        let plan = mnemoa_core::promote::plan_with(&ws, &config, d.as_deref(), ast_grep.as_deref());
        let written = if req.apply {
            mnemoa_core::promote::apply(&ws, &plan).map_err(ApiError::write)?
        } else {
            Vec::new()
        };
        with_written(&plan, written)
    })
    .await?;
    Ok(Json(v))
}

#[derive(Debug, Deserialize)]
pub struct ForgetRequest {
    #[serde(default = "dot")]
    pub path: String,
    #[serde(default)]
    pub apply: bool,
}

async fn forget_handler(
    State(s): State<Arc<AppState>>,
    Json(req): Json<ForgetRequest>,
) -> ApiResult<Value> {
    s.requests.fetch_add(1, Ordering::Relaxed);
    check_write(&s, req.apply)?;
    let path = resolve(&s.root, &req.path)?;
    let config = repo_config(&s, &path)?;
    let v = blocking(move || {
        let ws = scan::load_workspace(&path, &config).map_err(|e| ApiError::bad(e.to_string()))?;
        let ast_grep = mnemoa_core::drafter::find_ast_grep();
        let plan = mnemoa_core::forget::plan(
            &ws,
            &config,
            &mnemoa_core::ledger::today(),
            ast_grep.as_deref(),
        );
        let written = if req.apply {
            mnemoa_core::forget::apply(&ws, &plan).map_err(ApiError::write)?
        } else {
            Vec::new()
        };
        with_written(&plan, written)
    })
    .await?;
    Ok(Json(v))
}

async fn rules_handler(
    State(s): State<Arc<AppState>>,
) -> Json<&'static [mnemoa_core::rules::RuleInfo]> {
    s.requests.fetch_add(1, Ordering::Relaxed);
    Json(&mnemoa_core::rules::RULES[..])
}

async fn rule_handler(
    State(s): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<mnemoa_core::rules::RuleInfo>, ApiError> {
    s.requests.fetch_add(1, Ordering::Relaxed);
    mnemoa_core::rules::info(&id)
        .copied()
        .map(Json)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("unknown rule `{id}`")))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StatsResponse {
    pub requests: u64,
    pub scans: u64,
    pub findings_by_rule: BTreeMap<String, u64>,
    pub uptime_secs: u64,
}

async fn stats_handler(State(s): State<Arc<AppState>>) -> Json<StatsResponse> {
    let requests = s.requests.fetch_add(1, Ordering::Relaxed) + 1;
    Json(StatsResponse {
        requests,
        scans: s.scans.load(Ordering::Relaxed),
        findings_by_rule: s
            .findings_by_rule
            .lock()
            .map(|m| m.clone())
            .unwrap_or_default(),
        uptime_secs: s.started.elapsed().as_secs(),
    })
}

/// The application router.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/scan", post(scan_handler))
        .route("/budget", post(budget_handler))
        .route("/drift", post(drift_handler))
        .route("/promote", post(promote_handler))
        .route("/forget", post(forget_handler))
        .route("/rules", get(rules_handler))
        .route("/rules/{id}", get(rule_handler))
        .route("/stats", get(stats_handler))
        .with_state(state)
}
