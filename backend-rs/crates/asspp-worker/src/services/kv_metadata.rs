use asspp_core::types::DownloadTask;
use worker::*;

/// KV-backed task metadata storage.
pub struct KvMetadata {
  kv: kv::KvStore,
}

impl KvMetadata {
  pub fn new(kv: kv::KvStore) -> Self {
    Self { kv }
  }

  fn task_key(id: &str) -> String {
    format!("task:{}", id)
  }

  fn account_index_key(account_hash: &str) -> String {
    format!("account_tasks:{}", account_hash)
  }

  pub async fn get_task(&self, id: &str) -> Result<Option<DownloadTask>> {
    let key = Self::task_key(id);
    match self.kv.get(&key).json::<DownloadTask>().await? {
      Some(task) => Ok(Some(task)),
      None => Ok(None),
    }
  }

  pub async fn put_task(&self, task: &DownloadTask) -> Result<()> {
    let key = Self::task_key(&task.id);
    let data = serde_json::to_string(task)
      .map_err(|e| Error::RustError(format!("Serialize: {}", e)))?;
    self.kv.put(&key, data)?.execute().await?;

    // Update account index
    self.add_to_account_index(&task.account_hash, &task.id).await?;

    Ok(())
  }

  pub async fn delete_task(&self, id: &str) -> Result<bool> {
    let task = self.get_task(id).await?;
    let key = Self::task_key(id);
    self.kv.delete(&key).await?;

    // Remove from account index
    if let Some(task) = task {
      self
        .remove_from_account_index(&task.account_hash, id)
        .await?;
      return Ok(true);
    }
    Ok(false)
  }

  pub async fn list_tasks(&self, account_hashes: &[String]) -> Result<Vec<DownloadTask>> {
    let mut tasks = Vec::new();
    for hash in account_hashes {
      let ids = self.get_account_task_ids(hash).await?;
      for id in ids {
        if let Some(task) = self.get_task(&id).await? {
          tasks.push(task);
        }
      }
    }
    Ok(tasks)
  }

  pub async fn list_all_tasks(&self) -> Result<Vec<DownloadTask>> {
    let listed = self.kv.list().prefix("task:".to_string()).execute().await?;
    let mut tasks = Vec::new();
    for key in listed.keys {
      if let Some(task) = self.kv.get(&key.name).json::<DownloadTask>().await? {
        tasks.push(task);
      }
    }
    Ok(tasks)
  }

  async fn get_account_task_ids(&self, account_hash: &str) -> Result<Vec<String>> {
    let key = Self::account_index_key(account_hash);
    match self.kv.get(&key).json::<Vec<String>>().await? {
      Some(ids) => Ok(ids),
      None => Ok(vec![]),
    }
  }

  async fn add_to_account_index(&self, account_hash: &str, task_id: &str) -> Result<()> {
    let key = Self::account_index_key(account_hash);
    let mut ids = self.get_account_task_ids(account_hash).await?;
    if !ids.contains(&task_id.to_string()) {
      ids.push(task_id.to_string());
      let data = serde_json::to_string(&ids)
        .map_err(|e| Error::RustError(format!("Serialize index: {}", e)))?;
      self.kv.put(&key, data)?.execute().await?;
    }
    Ok(())
  }

  async fn remove_from_account_index(&self, account_hash: &str, task_id: &str) -> Result<()> {
    let key = Self::account_index_key(account_hash);
    let mut ids = self.get_account_task_ids(account_hash).await?;
    ids.retain(|id| id != task_id);
    let data = serde_json::to_string(&ids)
      .map_err(|e| Error::RustError(format!("Serialize index: {}", e)))?;
    self.kv.put(&key, data)?.execute().await?;
    Ok(())
  }
}
