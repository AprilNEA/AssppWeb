use axum::{
  body::Body,
  extract::{Path, State},
  http::{header, StatusCode},
  response::{IntoResponse, Json, Response},
  routing::get,
  Router,
};
use serde_json::Value;
use tokio_util::io::ReaderStream;

use asspp_core::manifest::{build_manifest, WHITE_PNG};
use asspp_core::security::path_within_base;
use asspp_core::types::TaskStatus;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
  Router::new()
    .route("/install/{id}/manifest.plist", get(manifest))
    .route("/install/{id}/url", get(install_url))
    .route("/install/{id}/payload.ipa", get(payload))
    .route("/install/{id}/icon-small.png", get(icon_small))
    .route("/install/{id}/icon-large.png", get(icon_large))
}

fn get_base_url(state: &AppState, headers: &axum::http::HeaderMap) -> String {
  let configured = state.config.public_base_url.trim().trim_end_matches('/');
  if !configured.is_empty() {
    return configured.to_string();
  }

  let forwarded_proto = headers
    .get("x-forwarded-proto")
    .and_then(|v| v.to_str().ok())
    .unwrap_or("http");
  let proto = if forwarded_proto == "https" {
    "https"
  } else {
    "http"
  };

  let host = headers
    .get("host")
    .and_then(|v| v.to_str().ok())
    .unwrap_or("localhost");

  let sanitized_host: String = host
    .chars()
    .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-' || *c == ':')
    .collect();

  format!("{}://{}", proto, sanitized_host)
}

async fn manifest(
  State(state): State<AppState>,
  Path(id): Path<String>,
  headers: axum::http::HeaderMap,
) -> Result<Response, (StatusCode, Json<Value>)> {
  let tasks = state.tasks.read().await;
  let task = tasks
    .values()
    .find(|t| t.id == id && t.status == TaskStatus::Completed && t.file_path.is_some())
    .ok_or_else(|| {
      (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Package not found"})),
      )
    })?;

  let base_url = get_base_url(&state, &headers);
  let payload_url = format!("{}/api/install/{}/payload.ipa", base_url, id);
  let small_icon_url = format!("{}/api/install/{}/icon-small.png", base_url, id);
  let large_icon_url = format!("{}/api/install/{}/icon-large.png", base_url, id);

  let xml = build_manifest(&task.software, &payload_url, &small_icon_url, &large_icon_url);

  Ok(
    (
      StatusCode::OK,
      [(header::CONTENT_TYPE, "application/xml")],
      xml,
    )
      .into_response(),
  )
}

async fn install_url(
  State(state): State<AppState>,
  Path(id): Path<String>,
  headers: axum::http::HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
  let tasks = state.tasks.read().await;
  let _task = tasks
    .values()
    .find(|t| t.id == id && t.status == TaskStatus::Completed && t.file_path.is_some())
    .ok_or_else(|| {
      (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Package not found"})),
      )
    })?;

  let base_url = get_base_url(&state, &headers);
  let manifest_url = format!("{}/api/install/{}/manifest.plist", base_url, id);
  let install_url_str = format!(
    "itms-services://?action=download-manifest&url={}",
    urlencoding::encode(&manifest_url)
  );

  Ok(Json(serde_json::json!({
    "installUrl": install_url_str,
    "manifestUrl": manifest_url,
  })))
}

async fn payload(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Response, (StatusCode, Json<Value>)> {
  let tasks = state.tasks.read().await;
  let task = tasks
    .values()
    .find(|t| t.id == id && t.status == TaskStatus::Completed)
    .ok_or_else(|| {
      (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Package not found"})),
      )
    })?;

  let file_path = task.file_path.as_ref().ok_or_else(|| {
    (
      StatusCode::NOT_FOUND,
      Json(serde_json::json!({"error": "Package not found"})),
    )
  })?;

  if !std::path::Path::new(file_path).exists() {
    return Err((
      StatusCode::NOT_FOUND,
      Json(serde_json::json!({"error": "Package not found"})),
    ));
  }

  // Path safety
  let resolved = std::fs::canonicalize(file_path).map_err(|_| {
    (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Package not found"})))
  })?;
  let packages_base = std::fs::canonicalize(state.config.packages_dir()).map_err(|_| {
    (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Package not found"})))
  })?;

  if !path_within_base(&resolved, &packages_base) {
    return Err((
      StatusCode::FORBIDDEN,
      Json(serde_json::json!({"error": "Access denied"})),
    ));
  }

  let metadata = tokio::fs::metadata(&resolved).await.map_err(|_| {
    (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Package not found"})))
  })?;

  let file = tokio::fs::File::open(&resolved).await.map_err(|_| {
    (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Package not found"})))
  })?;

  let stream = ReaderStream::new(file);
  let body = Body::from_stream(stream);

  Ok(
    (
      [
        (header::CONTENT_TYPE, "application/octet-stream".to_string()),
        (header::CONTENT_LENGTH, metadata.len().to_string()),
      ],
      body,
    )
      .into_response(),
  )
}

async fn icon_small() -> impl IntoResponse {
  (
    [
      (header::CONTENT_TYPE, "image/png"),
      (header::CONTENT_LENGTH, "70"),
    ],
    WHITE_PNG,
  )
}

async fn icon_large() -> impl IntoResponse {
  (
    [
      (header::CONTENT_TYPE, "image/png"),
      (header::CONTENT_LENGTH, "70"),
    ],
    WHITE_PNG,
  )
}
