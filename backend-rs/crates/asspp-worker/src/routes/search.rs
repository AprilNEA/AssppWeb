use asspp_core::search::{map_lookup_result, map_search_results};
use worker::*;

pub async fn search(req: Request, _ctx: RouteContext<()>) -> Result<Response> {
  let url = req.url()?;
  let query = url.query().unwrap_or_default();
  let itunes_url = format!("https://itunes.apple.com/search?{}", query);

  let mut init = RequestInit::new();
  init.method = Method::Get;
  let itunes_req = Request::new_with_init(&itunes_url, &init)?;
  let mut resp = Fetch::Request(itunes_req).send().await?;

  let data: serde_json::Value = resp.json().await?;
  let results = map_search_results(&data);
  Response::from_json(&results)
}

pub async fn lookup(req: Request, _ctx: RouteContext<()>) -> Result<Response> {
  let url = req.url()?;
  let query = url.query().unwrap_or_default();
  let itunes_url = format!("https://itunes.apple.com/lookup?{}", query);

  let mut init = RequestInit::new();
  init.method = Method::Get;
  let itunes_req = Request::new_with_init(&itunes_url, &init)?;
  let mut resp = Fetch::Request(itunes_req).send().await?;

  let data: serde_json::Value = resp.json().await?;
  match map_lookup_result(&data) {
    Some(sw) => Response::from_json(&sw),
    None => Response::from_json(&serde_json::Value::Null),
  }
}
