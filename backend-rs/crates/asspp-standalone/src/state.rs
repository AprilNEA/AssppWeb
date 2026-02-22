use asspp_core::config::Config;
use asspp_core::types::{DownloadTask, TaskStatus};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, RwLock};

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
  pub config: Config,
  pub tasks: Arc<RwLock<HashMap<String, DownloadTask>>>,
  pub abort_handles: Arc<Mutex<HashMap<String, tokio::sync::watch::Sender<bool>>>>,
  pub progress_tx: Arc<RwLock<HashMap<String, broadcast::Sender<DownloadTask>>>>,
}

impl AppState {
  pub async fn new(config: Config) -> Self {
    let state = Self {
      config: config.clone(),
      tasks: Arc::new(RwLock::new(HashMap::new())),
      abort_handles: Arc::new(Mutex::new(HashMap::new())),
      progress_tx: Arc::new(RwLock::new(HashMap::new())),
    };

    // Load persisted tasks
    state.load_tasks().await;

    // Clean orphaned packages
    state.clean_orphaned_packages().await;

    state
  }

  async fn load_tasks(&self) {
    let tasks_file = self.config.tasks_file();
    let data = match tokio::fs::read_to_string(&tasks_file).await {
      Ok(d) => d,
      Err(_) => return,
    };

    let items: Vec<serde_json::Value> = match serde_json::from_str(&data) {
      Ok(v) => v,
      Err(_) => return,
    };

    let mut tasks = self.tasks.write().await;
    for item in items {
      let id = item.get("id").and_then(|v| v.as_str()).unwrap_or_default();
      let status = item
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
      let file_path = item
        .get("filePath")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

      if id.is_empty() || status != "completed" || file_path.is_empty() {
        continue;
      }

      // Only restore if IPA file still exists
      if !Path::new(file_path).exists() {
        continue;
      }

      // Parse the full task
      if let Ok(mut task) = serde_json::from_value::<DownloadTask>(item) {
        task.status = TaskStatus::Completed;
        task.progress = 100;
        task.speed = "0 B/s".into();
        task.download_url = String::new();
        task.sinfs = vec![];
        task.itunes_metadata = None;
        tasks.insert(task.id.clone(), task);
      }
    }

    tracing::info!("Loaded {} completed tasks from disk", tasks.len());
  }

  async fn clean_orphaned_packages(&self) {
    let packages_dir = self.config.packages_dir();
    if !Path::new(&packages_dir).exists() {
      return;
    }

    let known_paths: HashSet<String> = {
      let tasks = self.tasks.read().await;
      tasks
        .values()
        .filter_map(|t| t.file_path.clone())
        .map(|p| {
          std::fs::canonicalize(&p)
            .unwrap_or_else(|_| p.into())
            .to_string_lossy()
            .to_string()
        })
        .collect()
    };

    walk_and_clean(&packages_dir, &packages_dir, &known_paths);
  }

  pub async fn persist_tasks(&self) {
    let tasks = self.tasks.read().await;
    let completed: Vec<serde_json::Value> = tasks
      .values()
      .filter(|t| t.status == TaskStatus::Completed && t.file_path.is_some())
      .filter_map(|t| {
        serde_json::to_value(&t.to_persisted()).ok()
      })
      .collect();

    let data = serde_json::to_string_pretty(&completed).unwrap_or_else(|_| "[]".into());
    if let Err(e) = tokio::fs::write(&self.config.tasks_file(), data).await {
      tracing::error!("Failed to persist tasks: {}", e);
    }
  }

  pub async fn notify_progress(&self, task: &DownloadTask) {
    let txs = self.progress_tx.read().await;
    if let Some(tx) = txs.get(&task.id) {
      let _ = tx.send(task.clone());
    }
  }

  pub async fn get_or_create_progress_tx(
    &self,
    task_id: &str,
  ) -> broadcast::Sender<DownloadTask> {
    let txs = self.progress_tx.read().await;
    if let Some(tx) = txs.get(task_id) {
      return tx.clone();
    }
    drop(txs);
    let (tx, _) = broadcast::channel(64);
    let mut txs = self.progress_tx.write().await;
    txs.insert(task_id.to_string(), tx.clone());
    tx
  }
}

fn walk_and_clean(dir: &str, packages_base: &str, known: &HashSet<String>) {
  let entries = match std::fs::read_dir(dir) {
    Ok(e) => e,
    Err(_) => return,
  };

  for entry in entries.flatten() {
    let path = entry.path();
    if path.is_dir() {
      walk_and_clean(&path.to_string_lossy(), packages_base, known);
      // Remove empty dirs
      if std::fs::read_dir(&path).map(|mut d| d.next().is_none()).unwrap_or(false) {
        let _ = std::fs::remove_dir(&path);
      }
    } else if path.is_file() {
      let canonical = std::fs::canonicalize(&path)
        .unwrap_or_else(|_| path.clone())
        .to_string_lossy()
        .to_string();
      if !known.contains(&canonical) {
        let _ = std::fs::remove_file(&path);
      }
    }
  }
}
