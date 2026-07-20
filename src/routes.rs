use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::json;
use tokio::sync::RwLock;

use crate::config::Config;
use crate::manifest::{Manifest, Rendered, VersionEntry};

/// The fixed URL older PC WheelWizard clients reinstall from
const LEGACY_REINSTALL_ROUTE: &str = "/RetroRewind/zip/RetroRewind.zip";

pub struct AppState {
    pub config: Config,
    inner: RwLock<Inner>,
}

struct Inner {
    manifest: Manifest,
    rendered: Arc<Rendered>,
}

impl AppState {
    pub async fn load(config: Config) -> anyhow::Result<Arc<Self>> {
        let text = tokio::fs::read_to_string(&config.manifest_path)
            .await
            .with_context(|| format!("reading manifest from {}", config.manifest_path.display()))?;
        let manifest: Manifest = serde_json::from_str(&text)
            .with_context(|| format!("parsing manifest from {}", config.manifest_path.display()))?;
        manifest
            .validate()
            .map_err(|error| anyhow::anyhow!("invalid manifest: {error}"))?;

        let rendered = Arc::new(manifest.render());
        if rendered.install_txt.is_none() {
            tracing::warn!(
                "no version has a full_download, so RetroRewindInstall.txt \
                 and {LEGACY_REINSTALL_ROUTE} will return 404"
            );
        }
        tracing::info!(
            versions = manifest.versions.len(),
            "loaded {}",
            config.manifest_path.display()
        );

        Ok(Arc::new(Self {
            config,
            inner: RwLock::new(Inner { manifest, rendered }),
        }))
    }

    async fn rendered(&self) -> Arc<Rendered> {
        Arc::clone(&self.inner.read().await.rendered)
    }

    /// Validate, persist, then swap in. A manifest that fails either step
    /// leaves the served files untouched.
    async fn store(&self, inner: &mut Inner, manifest: Manifest) -> Result<(), AppError> {
        manifest.validate().map_err(AppError::Invalid)?;
        persist(&self.config.manifest_path, &manifest)
            .await
            .map_err(AppError::Internal)?;

        inner.rendered = Arc::new(manifest.render());
        inner.manifest = manifest;
        Ok(())
    }
}

/// Write to a temp file and rename, so a crash mid-write can't truncate the
/// manifest
async fn persist(path: &Path, manifest: &Manifest) -> anyhow::Result<()> {
    let mut json = serde_json::to_vec_pretty(manifest)?;
    json.push(b'\n');

    let temp = path.with_extension("json.tmp");
    tokio::fs::write(&temp, &json)
        .await
        .with_context(|| format!("writing {}", temp.display()))?;
    tokio::fs::rename(&temp, path)
        .await
        .with_context(|| format!("renaming {} to {}", temp.display(), path.display()))?;
    Ok(())
}

pub fn router(state: Arc<AppState>) -> Router {
    let admin = Router::new()
        .route("/manifest", get(get_manifest).put(put_manifest))
        .route("/versions", post(post_version))
        .route_layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            require_admin,
        ));

    Router::new()
        .route("/", get(get_root))
        .route("/RetroRewind/RetroRewindVersion.txt", get(version_txt))
        .route("/RetroRewind/RetroRewindDelete.txt", get(delete_txt))
        .route("/RetroRewind/RetroRewindInstall.txt", get(install_txt))
        .route(LEGACY_REINSTALL_ROUTE, get(legacy_reinstall_zip))
        .nest("/admin", admin)
        .with_state(state)
}

async fn version_txt(State(state): State<Arc<AppState>>) -> Response {
    text(state.rendered().await.version_txt.clone())
}

async fn delete_txt(State(state): State<Arc<AppState>>) -> Response {
    text(state.rendered().await.delete_txt.clone())
}

async fn install_txt(State(state): State<Arc<AppState>>) -> Result<Response, AppError> {
    match state.rendered().await.install_txt.clone() {
        Some(url) => Ok(text(url)),
        None => Err(AppError::NotFound("no full download is published")),
    }
}

/// Old PC clients reinstall from this fixed path instead of reading
/// `RetroRewindInstall.txt`, so send them to whatever that file points at.
///
/// Temporary: drop this route once those clients are gone.
///
/// 302 rather than a permanent redirect, because the target moves with every
/// full release and a 301 would be cached against us. 302 rather than axum's
/// 307/303 helpers because it is the status every ancient HTTP client
/// understands, and these are by definition old clients.
async fn legacy_reinstall_zip(State(state): State<Arc<AppState>>) -> Result<Response, AppError> {
    let Some(url) = state.rendered().await.install_txt.clone() else {
        return Err(AppError::NotFound("no full download is published"));
    };

    // The URL comes from the manifest, which rejects whitespace and control
    // characters, so it is safe to put in a header.
    let location = HeaderValue::from_str(&url).map_err(|_| {
        AppError::Internal(anyhow::anyhow!(
            "full download url is not a valid header: {url:?}"
        ))
    })?;
    Ok((StatusCode::FOUND, [(header::LOCATION, location)]).into_response())
}

async fn get_manifest(State(state): State<Arc<AppState>>) -> Json<Manifest> {
    Json(state.inner.read().await.manifest.clone())
}

// This is required because WheelWIzard queries root and a non-200 response will make it think the server is down
async fn get_root(State(_): State<Arc<AppState>>) -> Response {
    text("OK".to_owned())
}

/// Replace the whole manifest.
async fn put_manifest(
    State(state): State<Arc<AppState>>,
    Json(manifest): Json<Manifest>,
) -> Result<Json<Summary>, AppError> {
    let mut inner = state.inner.write().await;
    state.store(&mut inner, manifest).await?;
    Ok(Json(Summary::of(&inner)))
}

/// Append one release. Its version must sort after every existing one.
async fn post_version(
    State(state): State<Arc<AppState>>,
    Json(entry): Json<VersionEntry>,
) -> Result<(StatusCode, Json<Summary>), AppError> {
    let mut inner = state.inner.write().await;

    let mut manifest = inner.manifest.clone();
    manifest.versions.push(entry);
    state.store(&mut inner, manifest).await?;

    Ok((StatusCode::CREATED, Json(Summary::of(&inner))))
}

#[derive(Serialize)]
pub struct Summary {
    versions: usize,
    latest: Option<String>,
    install: Option<String>,
}

impl Summary {
    fn of(inner: &Inner) -> Self {
        Self {
            versions: inner.manifest.versions.len(),
            latest: inner
                .manifest
                .versions
                .last()
                .map(|entry| entry.version.clone()),
            install: inner.rendered.install_txt.clone(),
        }
    }
}

async fn require_admin(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));

    match presented {
        Some(token) if constant_time_eq(token.as_bytes(), state.config.admin_token.as_bytes()) => {
            Ok(next.run(request).await)
        }
        _ => Err(AppError::Unauthorized),
    }
}

/// Compare without leaking the token through timing. Length is not secret.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .fold(0, |difference, (x, y)| difference | (x ^ y))
            == 0
}

fn text(body: String) -> Response {
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response()
}

pub enum AppError {
    Unauthorized,
    NotFound(&'static str),
    Invalid(String),
    Internal(anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "missing or invalid bearer token".to_owned(),
            ),
            Self::NotFound(message) => (StatusCode::NOT_FOUND, message.to_owned()),
            Self::Invalid(message) => (StatusCode::BAD_REQUEST, message),
            Self::Internal(error) => {
                tracing::error!("{error:#}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_owned(),
                )
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matches_normal_equality() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secrez"));
        assert!(!constant_time_eq(b"secret", b"secret-longer"));
        assert!(!constant_time_eq(b"", b"x"));
        assert!(constant_time_eq(b"", b""));
    }
}
