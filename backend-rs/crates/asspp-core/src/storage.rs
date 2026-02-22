use crate::types::DownloadTask;

/// Blob storage trait for IPA files (platform-specific implementation).
pub trait BlobStorage: Send + Sync {
  /// Store data from bytes, returning the total size written.
  fn put(
    &self,
    key: &str,
    data: &[u8],
  ) -> impl std::future::Future<Output = Result<u64, String>> + Send;

  /// Get the raw bytes for a key.
  fn get(
    &self,
    key: &str,
  ) -> impl std::future::Future<Output = Result<Option<Vec<u8>>, String>> + Send;

  /// Get the size of an object.
  fn size(
    &self,
    key: &str,
  ) -> impl std::future::Future<Output = Result<Option<u64>, String>> + Send;

  /// Delete an object. Returns true if it existed.
  fn delete(&self, key: &str) -> impl std::future::Future<Output = Result<bool, String>> + Send;

  /// Check if a key exists.
  fn exists(&self, key: &str) -> impl std::future::Future<Output = Result<bool, String>> + Send;
}

/// Task metadata storage trait (platform-specific implementation).
pub trait TaskMetadata: Send + Sync {
  /// Get a task by ID.
  fn get_task(
    &self,
    id: &str,
  ) -> impl std::future::Future<Output = Result<Option<DownloadTask>, String>> + Send;

  /// Store or update a task.
  fn put_task(
    &self,
    task: &DownloadTask,
  ) -> impl std::future::Future<Output = Result<(), String>> + Send;

  /// Delete a task by ID. Returns true if it existed.
  fn delete_task(
    &self,
    id: &str,
  ) -> impl std::future::Future<Output = Result<bool, String>> + Send;

  /// List tasks filtered by account hashes.
  fn list_tasks(
    &self,
    account_hashes: &[String],
  ) -> impl std::future::Future<Output = Result<Vec<DownloadTask>, String>> + Send;

  /// List all tasks.
  fn list_all_tasks(
    &self,
  ) -> impl std::future::Future<Output = Result<Vec<DownloadTask>, String>> + Send;
}
