use asspp_core::download::{build_ipa_path, build_task_dir, new_task};
use asspp_core::security::{format_speed, path_within_base, validate_download_url, MAX_DOWNLOAD_SIZE};
use asspp_core::types::{CreateDownloadRequest, DownloadTask, TaskStatus};
use std::time::Instant;
use tokio::io::AsyncWriteExt;

use crate::services::sinf_injector;
use crate::state::AppState;

/// Create a new download task and start the download.
pub async fn create_task(
  state: &AppState,
  req: CreateDownloadRequest,
) -> Result<DownloadTask, String> {
  let task = new_task(req);
  let task_id = task.id.clone();

  state.tasks.write().await.insert(task_id.clone(), task.clone());

  // Start download in background
  let state2 = state.clone();
  tokio::spawn(async move {
    start_download(&state2, &task_id).await;
  });

  Ok(task)
}

/// Pause an active download.
pub async fn pause_task(state: &AppState, id: &str) -> bool {
  let mut tasks = state.tasks.write().await;
  let task = match tasks.get_mut(id) {
    Some(t) if t.status == TaskStatus::Downloading => t,
    _ => return false,
  };

  task.status = TaskStatus::Paused;
  let task_clone = task.clone();
  drop(tasks);

  // Signal abort
  let mut handles = state.abort_handles.lock().await;
  if let Some(tx) = handles.remove(id) {
    let _ = tx.send(true);
  }

  state.notify_progress(&task_clone).await;
  true
}

/// Resume a paused download.
pub async fn resume_task(state: &AppState, id: &str) -> bool {
  {
    let tasks = state.tasks.read().await;
    match tasks.get(id) {
      Some(t) if t.status == TaskStatus::Paused => {}
      _ => return false,
    }
  }

  let state2 = state.clone();
  let id = id.to_string();
  tokio::spawn(async move {
    start_download(&state2, &id).await;
  });

  true
}

/// Delete a task and its associated file.
pub async fn delete_task(state: &AppState, id: &str) {
  // Signal abort if downloading
  {
    let mut handles = state.abort_handles.lock().await;
    if let Some(tx) = handles.remove(id) {
      let _ = tx.send(true);
    }
  }

  let task = state.tasks.write().await.remove(id);

  if let Some(task) = task {
    if let Some(file_path) = &task.file_path {
      let packages_dir = state.config.packages_dir();
      let resolved = std::fs::canonicalize(file_path)
        .unwrap_or_else(|_| file_path.into());
      let packages_base = std::fs::canonicalize(&packages_dir)
        .unwrap_or_else(|_| packages_dir.into());

      if path_within_base(&resolved, &packages_base) && resolved.exists() {
        let _ = tokio::fs::remove_file(&resolved).await;

        // Clean empty parent dirs
        let mut dir = resolved.parent().map(|p| p.to_path_buf());
        while let Some(d) = dir {
          if !d.starts_with(&packages_base) || d == packages_base {
            break;
          }
          match std::fs::read_dir(&d) {
            Ok(mut entries) => {
              if entries.next().is_none() {
                let _ = std::fs::remove_dir(&d);
                dir = d.parent().map(|p| p.to_path_buf());
              } else {
                break;
              }
            }
            Err(_) => break,
          }
        }
      }
    }
  }

  state.progress_tx.write().await.remove(id);
  state.persist_tasks().await;
}

async fn start_download(state: &AppState, task_id: &str) {
  // Set up abort signal
  let (abort_tx, mut abort_rx) = tokio::sync::watch::channel(false);
  state
    .abort_handles
    .lock()
    .await
    .insert(task_id.to_string(), abort_tx);

  // Update status to downloading
  {
    let mut tasks = state.tasks.write().await;
    if let Some(task) = tasks.get_mut(task_id) {
      task.status = TaskStatus::Downloading;
      task.progress = 0;
      task.speed = "0 B/s".into();
      task.error = None;
      state.notify_progress(task).await;
    } else {
      return;
    }
  }

  // Get task data we need
  let (download_url, sinfs, itunes_metadata, packages_dir, account_hash, bundle_id, version) = {
    let tasks = state.tasks.read().await;
    let task = match tasks.get(task_id) {
      Some(t) => t,
      None => return,
    };
    (
      task.download_url.clone(),
      task.sinfs.clone(),
      task.itunes_metadata.clone(),
      state.config.packages_dir(),
      task.account_hash.clone(),
      task.software.bundle_id.clone(),
      task.software.version.clone(),
    )
  };

  // Build file path
  let dir = match build_task_dir(&packages_dir, &account_hash, &bundle_id, &version) {
    Ok(d) => d,
    Err(e) => {
      fail_task(state, task_id, &e).await;
      return;
    }
  };

  // Verify path is within packages dir
  let resolved_dir = std::path::Path::new(&dir).to_path_buf();
  let packages_base = std::path::Path::new(&packages_dir).to_path_buf();
  if !resolved_dir.starts_with(&packages_base) {
    fail_task(state, task_id, "Invalid path").await;
    return;
  }

  if let Err(e) = tokio::fs::create_dir_all(&dir).await {
    fail_task(state, task_id, &format!("Failed to create directory: {}", e)).await;
    return;
  }

  let file_path = build_ipa_path(&dir, task_id);

  // Store file path
  {
    let mut tasks = state.tasks.write().await;
    if let Some(task) = tasks.get_mut(task_id) {
      task.file_path = Some(file_path.clone());
    }
  }

  // Re-validate URL
  if let Err(e) = validate_download_url(&download_url) {
    fail_task(state, task_id, &e).await;
    return;
  }

  // Download
  let client = reqwest::Client::builder()
    .timeout(std::time::Duration::from_secs(600))
    .build()
    .unwrap();

  let resp = match client.get(&download_url).send().await {
    Ok(r) => r,
    Err(e) => {
      if is_aborted(state, task_id).await {
        return;
      }
      fail_task(state, task_id, &format!("HTTP error: {}", e)).await;
      return;
    }
  };

  if !resp.status().is_success() {
    fail_task(
      state,
      task_id,
      &format!("HTTP {}: {}", resp.status().as_u16(), resp.status().canonical_reason().unwrap_or("Unknown")),
    )
    .await;
    return;
  }

  let content_length = resp.content_length().unwrap_or(0);
  if content_length > MAX_DOWNLOAD_SIZE {
    fail_task(state, task_id, "File too large").await;
    return;
  }

  let mut file = match tokio::fs::File::create(&file_path).await {
    Ok(f) => f,
    Err(e) => {
      fail_task(state, task_id, &format!("File create error: {}", e)).await;
      return;
    }
  };

  let mut downloaded: u64 = 0;
  let mut last_time = Instant::now();
  let mut last_bytes: u64 = 0;

  let mut stream = resp.bytes_stream();
  use futures_util::StreamExt;

  loop {
    tokio::select! {
      chunk = stream.next() => {
        match chunk {
          Some(Ok(bytes)) => {
            downloaded += bytes.len() as u64;

            if downloaded > MAX_DOWNLOAD_SIZE {
              fail_task(state, task_id, "Download exceeded maximum size").await;
              return;
            }

            if let Err(e) = file.write_all(&bytes).await {
              fail_task(state, task_id, &format!("Write error: {}", e)).await;
              return;
            }

            // Speed calculation
            let now = Instant::now();
            let elapsed_ms = now.duration_since(last_time).as_millis() as u64;
            if elapsed_ms >= 500 {
              let bytes_per_sec = ((downloaded - last_bytes) as f64 / elapsed_ms as f64) * 1000.0;
              let speed = format_speed(bytes_per_sec);
              last_time = now;
              last_bytes = downloaded;

              let progress = if content_length > 0 {
                ((downloaded as f64 / content_length as f64) * 100.0).round() as u8
              } else {
                0
              };

              let mut tasks = state.tasks.write().await;
              if let Some(task) = tasks.get_mut(task_id) {
                task.speed = speed;
                task.progress = progress;
                state.notify_progress(task).await;
              }
            }
          }
          Some(Err(e)) => {
            if is_aborted(state, task_id).await {
              return;
            }
            fail_task(state, task_id, &format!("Download error: {}", e)).await;
            return;
          }
          None => break, // Download complete
        }
      }
      _ = abort_rx.changed() => {
        // Aborted (paused or deleted)
        return;
      }
    }
  }

  // Flush file
  if let Err(e) = file.flush().await {
    fail_task(state, task_id, &format!("Flush error: {}", e)).await;
    return;
  }
  drop(file);

  // Remove abort handle
  state.abort_handles.lock().await.remove(task_id);

  // Inject SINFs
  if !sinfs.is_empty() {
    {
      let mut tasks = state.tasks.write().await;
      if let Some(task) = tasks.get_mut(task_id) {
        task.status = TaskStatus::Injecting;
        task.progress = 100;
        state.notify_progress(task).await;
      }
    }

    if let Err(e) = sinf_injector::inject(&sinfs, &file_path, itunes_metadata.as_deref()).await {
      fail_task(state, task_id, &format!("SINF injection failed: {}", e)).await;
      return;
    }
  }

  // Mark completed and strip secrets
  {
    let mut tasks = state.tasks.write().await;
    if let Some(task) = tasks.get_mut(task_id) {
      task.status = TaskStatus::Completed;
      task.progress = 100;
      task.download_url = String::new();
      task.sinfs = vec![];
      task.itunes_metadata = None;
      state.notify_progress(task).await;
    }
  }

  state.persist_tasks().await;
}

async fn fail_task(state: &AppState, task_id: &str, error: &str) {
  state.abort_handles.lock().await.remove(task_id);
  let mut tasks = state.tasks.write().await;
  if let Some(task) = tasks.get_mut(task_id) {
    task.status = TaskStatus::Failed;
    task.error = Some("Download failed".into());
    tracing::error!("Download {} failed: {}", task_id, error);
    state.notify_progress(task).await;
  }
}

async fn is_aborted(state: &AppState, task_id: &str) -> bool {
  let tasks = state.tasks.read().await;
  matches!(tasks.get(task_id), Some(t) if t.status == TaskStatus::Paused)
}
