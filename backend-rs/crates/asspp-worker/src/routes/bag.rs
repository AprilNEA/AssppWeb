use asspp_core::bag;
use worker::*;

pub async fn bag_handler(req: Request, _ctx: RouteContext<()>) -> Result<Response> {
  let url = req.url()?;
  let params: std::collections::HashMap<String, String> = url
    .query_pairs()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();

  let guid = match params.get("guid") {
    Some(g) if !g.is_empty() => g.clone(),
    _ => return Response::error("Missing guid parameter", 400),
  };

  if !bag::validate_guid(&guid) {
    return Response::error("Invalid guid format", 400);
  }

  let bag_url = bag::bag_url(&guid);

  let mut init = RequestInit::new();
  init.method = Method::Get;

  let headers = Headers::new();
  headers.set("User-Agent", bag::BAG_USER_AGENT)?;
  headers.set("Accept", "application/xml")?;
  init.headers = headers;

  let request = Request::new_with_init(&bag_url, &init)?;
  let mut resp = Fetch::Request(request).send().await?;

  if resp.status_code() >= 400 {
    return Response::error("Bag request failed", 502);
  }

  let body = resp.text().await?;

  if body.len() > bag::BAG_MAX_RESPONSE_BYTES {
    return Response::error("Bag response too large", 502);
  }

  match bag::extract_plist(&body) {
    Some(plist) => {
      let headers = Headers::new();
      headers.set("Content-Type", "text/xml")?;
      Ok(Response::ok(plist)?.with_headers(headers))
    }
    None => Response::error("No plist found in bag response", 502),
  }
}
