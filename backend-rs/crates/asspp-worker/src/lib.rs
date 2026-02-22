mod routes;
mod services;
mod durable_objects;

use worker::*;

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
  let router = Router::new();

  router
    // API routes
    .get_async("/api/search", routes::search::search)
    .get_async("/api/lookup", routes::search::lookup)
    .get_async("/api/bag", routes::bag::bag_handler)
    .post_async("/api/downloads", routes::downloads::create_download)
    .get_async("/api/downloads", routes::downloads::list_downloads)
    .get_async("/api/downloads/:id", routes::downloads::get_download)
    .get_async("/api/downloads/:id/progress", routes::downloads::progress_stream)
    .post_async("/api/downloads/:id/pause", routes::downloads::pause_download)
    .post_async("/api/downloads/:id/resume", routes::downloads::resume_download)
    .delete_async("/api/downloads/:id", routes::downloads::delete_download)
    .get_async("/api/packages", routes::packages::list_packages)
    .get_async("/api/packages/:id/file", routes::packages::download_file)
    .delete_async("/api/packages/:id", routes::packages::delete_package)
    .get_async("/api/install/:id/manifest.plist", routes::install::manifest)
    .get_async("/api/install/:id/url", routes::install::install_url)
    .get_async("/api/install/:id/payload.ipa", routes::install::payload)
    .get_async("/api/install/:id/icon-small.png", routes::install::icon_small)
    .get_async("/api/install/:id/icon-large.png", routes::install::icon_large)
    .get_async("/api/settings", routes::settings::settings)
    // Wisp WebSocket: upgrade to Durable Object
    .on_async("/wisp/", routes::wisp::wisp_handler)
    .on_async("/wisp/*path", routes::wisp::wisp_handler)
    .run(req, env)
    .await
}
