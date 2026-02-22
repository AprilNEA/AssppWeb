use axum::{
  extract::Query,
  http::{header, StatusCode},
  response::{IntoResponse, Json, Response},
  routing::get,
  Router,
};
use serde::Deserialize;
use std::time::Duration;

use asspp_core::bag;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
  Router::new().route("/bag", get(bag_handler))
}

#[derive(Deserialize)]
struct BagQuery {
  guid: Option<String>,
}

async fn bag_handler(
  Query(query): Query<BagQuery>,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
  let guid = match query.guid {
    Some(g) if !g.is_empty() => g,
    _ => {
      return Err((
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"error": "Missing guid parameter"})),
      ))
    }
  };

  if !bag::validate_guid(&guid) {
    return Err((
      StatusCode::BAD_REQUEST,
      Json(serde_json::json!({"error": "Invalid guid format"})),
    ));
  }

  let url = bag::bag_url(&guid);

  let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(bag::BAG_TIMEOUT_SECS))
    .build()
    .map_err(|e| {
      tracing::error!("Bag client error: {}", e);
      (
        StatusCode::BAD_GATEWAY,
        Json(serde_json::json!({"error": "Bag request failed"})),
      )
    })?;

  let resp = client
    .get(&url)
    .header("User-Agent", bag::BAG_USER_AGENT)
    .header("Accept", "application/xml")
    .send()
    .await
    .map_err(|e| {
      tracing::error!("Bag proxy error: {}", e);
      (
        StatusCode::BAD_GATEWAY,
        Json(serde_json::json!({"error": "Bag request failed"})),
      )
    })?;

  if !resp.status().is_success() {
    tracing::error!("Bag upstream returned HTTP {}", resp.status());
    return Err((
      StatusCode::BAD_GATEWAY,
      Json(serde_json::json!({"error": "Bag request failed"})),
    ));
  }

  let body = resp.text().await.map_err(|e| {
    tracing::error!("Bag read error: {}", e);
    (
      StatusCode::BAD_GATEWAY,
      Json(serde_json::json!({"error": "Bag request failed"})),
    )
  })?;

  if body.len() > bag::BAG_MAX_RESPONSE_BYTES {
    return Err((
      StatusCode::BAD_GATEWAY,
      Json(serde_json::json!({"error": "Bag response too large"})),
    ));
  }

  let plist = bag::extract_plist(&body).ok_or_else(|| {
    (
      StatusCode::BAD_GATEWAY,
      Json(serde_json::json!({"error": "No plist found in bag response"})),
    )
  })?;

  Ok(
    (
      StatusCode::OK,
      [(header::CONTENT_TYPE, "text/xml")],
      plist.to_string(),
    )
      .into_response(),
  )
}
