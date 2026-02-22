use worker::*;

/// R2-backed blob storage for IPA files.
pub struct R2Storage {
  bucket: Bucket,
}

impl R2Storage {
  pub fn new(bucket: Bucket) -> Self {
    Self { bucket }
  }

  pub async fn put(&self, key: &str, data: Vec<u8>) -> Result<u64> {
    let len = data.len() as u64;
    self.bucket.put(key, data).execute().await?;
    Ok(len)
  }

  pub async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
    match self.bucket.get(key).execute().await? {
      Some(obj) => {
        let body = obj.body().ok_or_else(|| Error::RustError("No body".into()))?;
        let bytes = body.bytes().await?;
        Ok(Some(bytes))
      }
      None => Ok(None),
    }
  }

  pub async fn get_range(&self, key: &str, offset: u64, length: u64) -> Result<Vec<u8>> {
    let range = Range::OffsetWithLength { offset, length };
    match self.bucket.get(key).range(range).execute().await? {
      Some(obj) => {
        let body = obj.body().ok_or_else(|| Error::RustError("No body".into()))?;
        Ok(body.bytes().await?)
      }
      None => Err(Error::RustError(format!("Key not found: {}", key))),
    }
  }

  pub async fn size(&self, key: &str) -> Result<Option<u64>> {
    match self.bucket.head(key).await? {
      Some(obj) => Ok(Some(obj.size())),
      None => Ok(None),
    }
  }

  pub async fn delete(&self, key: &str) -> Result<bool> {
    self.bucket.delete(key).await?;
    Ok(true)
  }

  pub async fn list(&self, prefix: &str) -> Result<Vec<String>> {
    let listed = self
      .bucket
      .list()
      .prefix(prefix.to_string())
      .execute()
      .await?;
    Ok(
      listed
        .objects()
        .iter()
        .map(|obj| obj.key())
        .collect(),
    )
  }
}
