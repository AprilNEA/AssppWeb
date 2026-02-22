use axum::{
  http::HeaderMap,
  response::Json,
  routing::get,
  Router,
};
use serde_json::Value;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
  Router::new().route("/settings", get(settings))
}

async fn settings(headers: HeaderMap) -> Json<Value> {
  let hostname = headers
    .get("x-forwarded-host")
    .or_else(|| headers.get("host"))
    .and_then(|v| v.to_str().ok())
    .unwrap_or("localhost");

  Json(serde_json::json!({
    "hostname": hostname,
    "version": "1.0.0",
  }))
}
