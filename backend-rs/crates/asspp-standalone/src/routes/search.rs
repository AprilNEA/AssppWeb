use axum::{
  extract::Query,
  http::StatusCode,
  response::Json,
  routing::get,
  Router,
};
use serde_json::Value;
use std::collections::HashMap;

use asspp_core::search::{map_lookup_result, map_search_results};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
  Router::new()
    .route("/search", get(search))
    .route("/lookup", get(lookup))
}

async fn search(
  Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
  let qs: String = params
    .iter()
    .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
    .collect::<Vec<_>>()
    .join("&");

  let url = format!("https://itunes.apple.com/search?{}", qs);

  let resp = reqwest::get(&url).await.map_err(|e| {
    tracing::error!("Search error: {}", e);
    (
      StatusCode::INTERNAL_SERVER_ERROR,
      Json(serde_json::json!({"error": "Search request failed"})),
    )
  })?;

  let data: Value = resp.json::<Value>().await.map_err(|e| {
    tracing::error!("Search parse error: {}", e);
    (
      StatusCode::INTERNAL_SERVER_ERROR,
      Json(serde_json::json!({"error": "Search request failed"})),
    )
  })?;

  let results = map_search_results(&data);
  Ok(Json(serde_json::to_value(&results).unwrap_or(Value::Array(vec![]))))
}

async fn lookup(
  Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
  let qs: String = params
    .iter()
    .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
    .collect::<Vec<_>>()
    .join("&");

  let url = format!("https://itunes.apple.com/lookup?{}", qs);

  let resp = reqwest::get(&url).await.map_err(|e| {
    tracing::error!("Lookup error: {}", e);
    (
      StatusCode::INTERNAL_SERVER_ERROR,
      Json(serde_json::json!({"error": "Lookup request failed"})),
    )
  })?;

  let data: Value = resp.json::<Value>().await.map_err(|e| {
    tracing::error!("Lookup parse error: {}", e);
    (
      StatusCode::INTERNAL_SERVER_ERROR,
      Json(serde_json::json!({"error": "Lookup request failed"})),
    )
  })?;

  match map_lookup_result(&data) {
    Some(sw) => Ok(Json(serde_json::to_value(&sw).unwrap_or(Value::Null))),
    None => Ok(Json(Value::Null)),
  }
}
