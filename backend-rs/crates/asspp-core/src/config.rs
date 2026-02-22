/// Server configuration resolved from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
  pub port: u16,
  pub data_dir: String,
  pub public_base_url: String,
}

impl Config {
  pub fn from_env() -> Self {
    Self {
      port: std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8080),
      data_dir: std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".into()),
      public_base_url: std::env::var("PUBLIC_BASE_URL").unwrap_or_default(),
    }
  }

  pub fn packages_dir(&self) -> String {
    format!("{}/packages", self.data_dir)
  }

  pub fn tasks_file(&self) -> String {
    format!("{}/tasks.json", self.data_dir)
  }
}

impl Default for Config {
  fn default() -> Self {
    Self {
      port: 8080,
      data_dir: "./data".into(),
      public_base_url: String::new(),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_default_config() {
    let cfg = Config::default();
    assert_eq!(cfg.port, 8080);
    assert_eq!(cfg.data_dir, "./data");
    assert_eq!(cfg.public_base_url, "");
  }

  #[test]
  fn test_packages_dir() {
    let cfg = Config {
      data_dir: "/data".into(),
      ..Config::default()
    };
    assert_eq!(cfg.packages_dir(), "/data/packages");
    assert_eq!(cfg.tasks_file(), "/data/tasks.json");
  }
}
