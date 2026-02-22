mod routes;
mod services;
mod state;

use asspp_core::config::Config;
use axum::Router;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tower_http::services::{ServeDir, ServeFile};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
  tracing_subscriber::fmt()
    .with_env_filter(
      EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
    )
    .init();

  let config = Config::from_env();
  let addr = SocketAddr::from(([0, 0, 0, 0], config.port));

  // Ensure data directories exist
  let packages_dir = config.packages_dir();
  tokio::fs::create_dir_all(&packages_dir)
    .await
    .expect("Failed to create packages directory");
  tokio::fs::create_dir_all(&config.data_dir)
    .await
    .expect("Failed to create data directory");

  // Initialize app state (loads persisted tasks)
  let app_state = state::AppState::new(config.clone()).await;

  // Build API routes
  let api = routes::api_router();

  // Determine public directory (relative to binary location or env)
  let public_dir = std::env::var("PUBLIC_DIR").unwrap_or_else(|_| "./public".into());
  let index_file = format!("{}/index.html", &public_dir);

  // SPA: serve static files, fallback to index.html
  let spa = ServeDir::new(&public_dir).fallback(ServeFile::new(&index_file));

  let app = Router::new()
    .nest("/api", api)
    .route(
      "/wisp/{*path}",
      axum::routing::any(routes::wisp::wisp_handler),
    )
    .with_state(app_state)
    .fallback_service(spa);

  let listener = TcpListener::bind(addr).await.expect("Failed to bind");
  tracing::info!("Server listening on {}", addr);
  tracing::info!(
    "Data directory: {}",
    std::fs::canonicalize(&config.data_dir)
      .unwrap_or_else(|_| config.data_dir.into())
      .display()
  );

  axum::serve(listener, app).await.expect("Server error");
}
