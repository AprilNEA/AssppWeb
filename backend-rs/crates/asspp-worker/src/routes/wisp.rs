use worker::*;

/// On Workers, Wisp WebSocket connections are routed to a Durable Object
/// that manages the TCP relay via the connect() API.
pub async fn wisp_handler(req: Request, ctx: RouteContext<()>) -> Result<Response> {
  // Get or create a Durable Object for this connection
  let namespace = ctx.durable_object("WISP_PROXY")?;

  // Use a unique ID per connection
  let id = namespace.unique_id()?;
  let stub = id.get_stub()?;

  // Forward the WebSocket upgrade to the Durable Object
  stub.fetch_with_request(req).await
}
