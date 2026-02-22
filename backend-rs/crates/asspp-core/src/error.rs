use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
  #[error("Missing required fields: {0}")]
  MissingFields(String),

  #[error("{0}")]
  BadRequest(String),

  #[error("Access denied")]
  AccessDenied,

  #[error("Not found: {0}")]
  NotFound(String),

  #[error("Bad gateway: {0}")]
  BadGateway(String),

  #[error("Internal server error")]
  Internal(#[from] Box<dyn std::error::Error + Send + Sync>),
}

impl AppError {
  pub fn status_code(&self) -> u16 {
    match self {
      AppError::MissingFields(_) | AppError::BadRequest(_) => 400,
      AppError::AccessDenied => 403,
      AppError::NotFound(_) => 404,
      AppError::BadGateway(_) => 502,
      AppError::Internal(_) => 500,
    }
  }
}
