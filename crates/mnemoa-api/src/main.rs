//! `mnemoa-api`: HTTP server for mnemoa.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use mnemoa_api::{AppState, router};
use mnemoa_core::config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Server config: MNEMOA_CONFIG (or --config <file>), then MNEMOA_* environment variables.
    let mut args = std::env::args().skip(1);
    let mut config_path = std::env::var_os("MNEMOA_CONFIG").map(std::path::PathBuf::from);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" => config_path = args.next().map(Into::into),
            "--version" | "-V" => {
                println!("mnemoa-api {}", mnemoa_core::VERSION);
                return Ok(());
            }
            "--help" | "-h" => {
                println!(
                    "mnemoa-api {}\n\nUsage: mnemoa-api [--config FILE]\n\nEnvironment: MNEMOA_ROOT, MNEMOA_PORT, MNEMOA_API_ALLOW_WRITE, MNEMOA_LLM_*, RUST_LOG",
                    mnemoa_core::VERSION
                );
                return Ok(());
            }
            other => anyhow::bail!("unknown argument `{other}` (try --help)"),
        }
    }
    let config = match &config_path {
        Some(p) => Config::load(std::path::Path::new("."), Some(p)),
        None => {
            let mut c = Config::default();
            c.apply_env(std::env::vars()).map(|_| c)
        }
    }
    .context("loading config")?;
    let port = config.api.port;
    let state =
        Arc::new(AppState::new(config).context("MNEMOA_ROOT must be an existing directory")?);
    tracing::info!(
        root = %state.root.display(),
        port,
        allow_write = state.config.api.allow_write,
        config = %state.config.source,
        "mnemoa-api starting"
    );
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
