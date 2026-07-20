mod config;
mod manifest;
mod routes;

use std::path::PathBuf;

use anyhow::Context;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("update_server_api=info")),
        )
        .init();

    let config_path = std::env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from("config.toml"), PathBuf::from);
    let config = config::Config::load(&config_path)?;

    let bind = config.bind;
    let state = routes::AppState::load(config).await?;
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding {bind}"))?;

    tracing::info!("listening on http://{bind}");
    axum::serve(listener, routes::router(state))
        .await
        .context("serving")?;
    Ok(())
}
