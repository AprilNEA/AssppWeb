use axum::{
  extract::{Path, Query, State},
  http::StatusCode,
  response::{
    sse::{Event, Sse},
    Json,
  },
  routing::{delete, get, post},
  Router,
};
use futures_util::stream::Stream;
use serde::Deserialize;
use serde_json::Value;
use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};

use asspp_core::download::validate_create_request;
use asspp_core::security::validate_download_url;
use asspp_core::types::CreateDownloadRequest;
use crate::services::download_manager;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
  Router::new()
    .route("/downloads", post(create_download))
    .route("/downloads", get(list_downloads))
    .route("/downloads/{id}", get(get_download))
    .route("/downloads/{id}/progress", get(progress_stream))
    .route("/downloads/{id}/pause", post(pause_download))
    .route("/downloads/{id}/resume", post(resume_download))
    .route("/downloads/{id}", delete(delete_download))
}

#[derive(Deserialize)]
struct AccountHashQuery {
  #[serde(rename = "accountHash")]
  account_hash: Option<String>,
  #[serde(rename = "accountHashes")]
  account_hashes: Option<String>,
}

fn require_account_hash(query: &AccountHashQuery, body_hash: Option<&str>) -> Result<String, (StatusCode, Json<Value>)> {
  let hash = query
    .account_hash
    .as_deref()
    .or(body_hash)
    .unwrap_or_default();

  if hash.len() < 8 {
    return Err((
      StatusCode::BAD_REQUEST,
      Json(serde_json::json!({"error": "Missing or invalid accountHash parameter"})),
    ));
  }
  Ok(hash.to_string())
}

fn verify_ownership(task_hash: &str, hash: &str) -> Result<(), (StatusCode, Json<Value>)> {
  if task_hash != hash {
    return Err((
      StatusCode::FORBIDDEN,
      Json(serde_json::json!({"error": "Access denied"})),
    ));
  }
  Ok(())
}

async fn create_download(
  State(state): State<AppState>,
  Json(body): Json<CreateDownloadRequest>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
  // Validate download URL separately for specific error
  if let Err(msg) = validate_download_url(&body.download_url) {
    return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": msg}))));
  }

  if let Err(msg) = validate_create_request(&body) {
    return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": msg}))));
  }

  let task = download_manager::create_task(&state, body).await.map_err(|e| {
    tracing::error!("Create download error: {}", e);
    (
      StatusCode::BAD_REQUEST,
      Json(serde_json::json!({"error": "Failed to create download"})),
    )
  })?;

  let file_exists = task.file_path.as_ref().map(|p| std::path::Path::new(p).exists()).unwrap_or(false);
  let sanitized = task.sanitize(file_exists);
  Ok((StatusCode::CREATED, Json(serde_json::to_value(&sanitized).unwrap())))
}

async fn list_downloads(
  State(state): State<AppState>,
  Query(query): Query<AccountHashQuery>,
) -> Json<Value> {
  let hashes_param = match &query.account_hashes {
    Some(h) if !h.is_empty() => h.clone(),
    _ => return Json(Value::Array(vec![])),
  };

  let hashes: std::collections::HashSet<&str> = hashes_param.split(',').filter(|s| !s.is_empty()).collect();
  if hashes.is_empty() {
    return Json(Value::Array(vec![]));
  }

  let tasks = state.tasks.read().await;
  let filtered: Vec<Value> = tasks
    .values()
    .filter(|t| hashes.contains(t.account_hash.as_str()))
    .map(|t| {
      let file_exists = t.file_path.as_ref().map(|p| std::path::Path::new(p).exists()).unwrap_or(false);
      serde_json::to_value(&t.sanitize(file_exists)).unwrap()
    })
    .collect();

  Json(Value::Array(filtered))
}

async fn get_download(
  State(state): State<AppState>,
  Path(id): Path<String>,
  Query(query): Query<AccountHashQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
  let hash = require_account_hash(&query, None)?;

  let tasks = state.tasks.read().await;
  let task = tasks.get(&id).ok_or_else(|| {
    (
      StatusCode::NOT_FOUND,
      Json(serde_json::json!({"error": "Download not found"})),
    )
  })?;

  verify_ownership(&task.account_hash, &hash)?;

  let file_exists = task.file_path.as_ref().map(|p| std::path::Path::new(p).exists()).unwrap_or(false);
  Ok(Json(serde_json::to_value(&task.sanitize(file_exists)).unwrap()))
}

async fn progress_stream(
  State(state): State<AppState>,
  Path(id): Path<String>,
  Query(query): Query<AccountHashQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, Json<Value>)> {
  let hash = require_account_hash(&query, None)?;

  // Check task exists and ownership
  let initial = {
    let tasks = state.tasks.read().await;
    let task = tasks.get(&id).ok_or_else(|| {
      (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Download not found"})),
      )
    })?;
    verify_ownership(&task.account_hash, &hash)?;
    task.clone()
  };

  let tx = state.get_or_create_progress_tx(&id).await;
  let rx = tx.subscribe();
  let file_exists = initial.file_path.as_ref().map(|p| std::path::Path::new(p).exists()).unwrap_or(false);
  let initial_data = serde_json::to_string(&initial.sanitize(file_exists)).unwrap();

  struct ProgressStream {
    initial: Option<String>,
    rx: tokio::sync::broadcast::Receiver<asspp_core::types::DownloadTask>,
  }

  impl Stream for ProgressStream {
    type Item = Result<Event, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
      // Send initial state first
      if let Some(data) = self.initial.take() {
        return Poll::Ready(Some(Ok(Event::default().data(data))));
      }

      // Poll for updates
      match self.rx.try_recv() {
        Ok(task) => {
          let file_exists = task.file_path.as_ref().map(|p| std::path::Path::new(p).exists()).unwrap_or(false);
          let data = serde_json::to_string(&task.sanitize(file_exists)).unwrap();
          Poll::Ready(Some(Ok(Event::default().data(data))))
        }
        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
          cx.waker().wake_by_ref();
          Poll::Pending
        }
        Err(_) => Poll::Ready(None),
      }
    }
  }

  Ok(Sse::new(ProgressStream {
    initial: Some(initial_data),
    rx,
  }))
}

async fn pause_download(
  State(state): State<AppState>,
  Path(id): Path<String>,
  Query(query): Query<AccountHashQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
  let hash = require_account_hash(&query, None)?;

  {
    let tasks = state.tasks.read().await;
    let task = tasks.get(&id).ok_or_else(|| {
      (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Download not found"})))
    })?;
    verify_ownership(&task.account_hash, &hash)?;
  }

  let success = download_manager::pause_task(&state, &id).await;
  if !success {
    return Err((
      StatusCode::BAD_REQUEST,
      Json(serde_json::json!({"error": "Cannot pause this download"})),
    ));
  }

  let tasks = state.tasks.read().await;
  let task = tasks.get(&id).unwrap();
  let file_exists = task.file_path.as_ref().map(|p| std::path::Path::new(p).exists()).unwrap_or(false);
  Ok(Json(serde_json::to_value(&task.sanitize(file_exists)).unwrap()))
}

async fn resume_download(
  State(state): State<AppState>,
  Path(id): Path<String>,
  Query(query): Query<AccountHashQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
  let hash = require_account_hash(&query, None)?;

  {
    let tasks = state.tasks.read().await;
    let task = tasks.get(&id).ok_or_else(|| {
      (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Download not found"})))
    })?;
    verify_ownership(&task.account_hash, &hash)?;
  }

  let success = download_manager::resume_task(&state, &id).await;
  if !success {
    return Err((
      StatusCode::BAD_REQUEST,
      Json(serde_json::json!({"error": "Cannot resume this download"})),
    ));
  }

  let tasks = state.tasks.read().await;
  let task = tasks.get(&id).unwrap();
  let file_exists = task.file_path.as_ref().map(|p| std::path::Path::new(p).exists()).unwrap_or(false);
  Ok(Json(serde_json::to_value(&task.sanitize(file_exists)).unwrap()))
}

async fn delete_download(
  State(state): State<AppState>,
  Path(id): Path<String>,
  Query(query): Query<AccountHashQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
  let hash = require_account_hash(&query, None)?;

  {
    let tasks = state.tasks.read().await;
    let task = tasks.get(&id).ok_or_else(|| {
      (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Download not found"})))
    })?;
    verify_ownership(&task.account_hash, &hash)?;
  }

  download_manager::delete_task(&state, &id).await;

  Ok(Json(serde_json::json!({"success": true})))
}
