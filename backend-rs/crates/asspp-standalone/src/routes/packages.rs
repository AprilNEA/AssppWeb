use axum::{
  body::Body,
  extract::{Path, Query, State},
  http::{header, StatusCode},
  response::{IntoResponse, Json, Response},
  routing::{delete, get},
  Router,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use tokio_util::io::ReaderStream;

use asspp_core::security::{path_within_base, sanitize_filename};
use asspp_core::types::TaskStatus;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
  Router::new()
    .route("/packages", get(list_packages))
    .route("/packages/{id}/file", get(download_file))
    .route("/packages/{id}", delete(delete_package))
}

#[derive(Deserialize)]
struct PackagesQuery {
  #[serde(rename = "accountHashes")]
  account_hashes: Option<String>,
  #[serde(rename = "accountHash")]
  account_hash: Option<String>,
}

async fn list_packages(
  State(state): State<AppState>,
  Query(query): Query<PackagesQuery>,
) -> Json<Value> {
  let hashes_param = match &query.account_hashes {
    Some(h) if !h.is_empty() => h.clone(),
    _ => return Json(Value::Array(vec![])),
  };

  let hashes: HashSet<&str> = hashes_param.split(',').filter(|s| !s.is_empty()).collect();
  if hashes.is_empty() {
    return Json(Value::Array(vec![]));
  }

  let tasks = state.tasks.read().await;
  let mut packages: Vec<Value> = Vec::new();

  for task in tasks.values() {
    if task.status != TaskStatus::Completed || !hashes.contains(task.account_hash.as_str()) {
      continue;
    }

    let file_path = match &task.file_path {
      Some(p) if std::path::Path::new(p).exists() => p.clone(),
      _ => continue,
    };

    let file_size = match std::fs::metadata(&file_path) {
      Ok(m) => m.len(),
      Err(_) => continue,
    };

    packages.push(serde_json::json!({
      "id": task.id,
      "software": task.software,
      "accountHash": task.account_hash,
      "fileSize": file_size,
      "createdAt": task.created_at,
    }));
  }

  Json(Value::Array(packages))
}

async fn download_file(
  State(state): State<AppState>,
  Path(id): Path<String>,
  Query(query): Query<PackagesQuery>,
) -> Result<Response, (StatusCode, Json<Value>)> {
  let account_hash = query.account_hash.as_deref().unwrap_or_default();
  if account_hash.len() < 8 {
    return Err((
      StatusCode::BAD_REQUEST,
      Json(serde_json::json!({"error": "Missing or invalid accountHash"})),
    ));
  }

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

  if task.account_hash != account_hash {
    return Err((
      StatusCode::FORBIDDEN,
      Json(serde_json::json!({"error": "Access denied"})),
    ));
  }

  let file_path = task.file_path.as_ref().ok_or_else(|| {
    (
      StatusCode::NOT_FOUND,
      Json(serde_json::json!({"error": "Package not found"})),
    )
  })?;

  // Path safety check
  let resolved = std::fs::canonicalize(file_path).map_err(|_| {
    (
      StatusCode::NOT_FOUND,
      Json(serde_json::json!({"error": "Package not found"})),
    )
  })?;
  let packages_base = std::fs::canonicalize(state.config.packages_dir()).map_err(|_| {
    (
      StatusCode::NOT_FOUND,
      Json(serde_json::json!({"error": "Package not found"})),
    )
  })?;

  if !path_within_base(&resolved, &packages_base) {
    return Err((
      StatusCode::FORBIDDEN,
      Json(serde_json::json!({"error": "Access denied"})),
    ));
  }

  let metadata = tokio::fs::metadata(&resolved).await.map_err(|_| {
    (
      StatusCode::NOT_FOUND,
      Json(serde_json::json!({"error": "Package not found"})),
    )
  })?;

  let safe_name = sanitize_filename(&task.software.name);
  let safe_version = sanitize_filename(&task.software.version);
  let filename = format!("{}_{}.ipa", safe_name, safe_version);

  let file = tokio::fs::File::open(&resolved).await.map_err(|_| {
    (
      StatusCode::NOT_FOUND,
      Json(serde_json::json!({"error": "Package not found"})),
    )
  })?;

  let stream = ReaderStream::new(file);
  let body = Body::from_stream(stream);

  Ok(
    (
      [
        (header::CONTENT_TYPE, "application/octet-stream".to_string()),
        (
          header::CONTENT_DISPOSITION,
          format!("attachment; filename=\"{}\"", filename),
        ),
        (header::CONTENT_LENGTH, metadata.len().to_string()),
      ],
      body,
    )
      .into_response(),
  )
}

async fn delete_package(
  State(state): State<AppState>,
  Path(id): Path<String>,
  Query(query): Query<PackagesQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
  let account_hash = query.account_hash.as_deref().unwrap_or_default();
  if account_hash.len() < 8 {
    return Err((
      StatusCode::BAD_REQUEST,
      Json(serde_json::json!({"error": "Missing or invalid accountHash"})),
    ));
  }

  let tasks = state.tasks.read().await;
  let task = tasks.get(&id).ok_or_else(|| {
    (
      StatusCode::NOT_FOUND,
      Json(serde_json::json!({"error": "Package not found"})),
    )
  })?;

  if task.account_hash != account_hash {
    return Err((
      StatusCode::FORBIDDEN,
      Json(serde_json::json!({"error": "Access denied"})),
    ));
  }

  let file_path = task.file_path.clone().ok_or_else(|| {
    (
      StatusCode::NOT_FOUND,
      Json(serde_json::json!({"error": "Package not found"})),
    )
  })?;

  // Path safety check
  let packages_dir = state.config.packages_dir();
  let resolved = std::fs::canonicalize(&file_path).unwrap_or_else(|_| file_path.into());
  let packages_base =
    std::fs::canonicalize(&packages_dir).unwrap_or_else(|_| packages_dir.into());

  if path_within_base(&resolved, &packages_base) && resolved.exists() {
    let _ = tokio::fs::remove_file(&resolved).await;

    // Clean empty parent dirs
    let mut dir = resolved.parent().map(|p| p.to_path_buf());
    while let Some(d) = dir {
      if !d.starts_with(&packages_base) || d == packages_base {
        break;
      }
      match std::fs::read_dir(&d) {
        Ok(mut entries) => {
          if entries.next().is_none() {
            let _ = std::fs::remove_dir(&d);
            dir = d.parent().map(|p| p.to_path_buf());
          } else {
            break;
          }
        }
        Err(_) => break,
      }
    }
  }

  Ok(Json(serde_json::json!({"success": true})))
}
