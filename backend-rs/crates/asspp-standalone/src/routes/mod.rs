pub mod bag;
pub mod downloads;
pub mod install;
pub mod packages;
pub mod search;
pub mod settings;
pub mod wisp;

use crate::state::AppState;
use axum::Router;

pub fn api_router() -> Router<AppState> {
  Router::new()
    .merge(search::router())
    .merge(bag::router())
    .merge(downloads::router())
    .merge(packages::router())
    .merge(install::router())
    .merge(settings::router())
}
