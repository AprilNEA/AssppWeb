use worker::*;

pub async fn settings(req: Request, _ctx: RouteContext<()>) -> Result<Response> {
  let hostname = req
    .headers()
    .get("host")
    .ok()
    .flatten()
    .unwrap_or_else(|| "localhost".to_string());

  Response::from_json(&serde_json::json!({
    "hostname": hostname,
    "version": "1.0.0",
  }))
}
