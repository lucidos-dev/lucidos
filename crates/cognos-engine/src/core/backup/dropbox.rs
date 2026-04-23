use super::{BackupEntry, BackupProvider, BoxError};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::PgPool;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

const BACKUP_FOLDER: &str = "/CognOS Backups";

pub struct DropboxBackupProvider {
    pool: PgPool,
    client: reqwest::Client,
    folder_ensured: Arc<Mutex<bool>>,
}

#[derive(Deserialize)]
struct DropboxFileMetadata {
    id: String,
}

#[derive(Deserialize)]
struct DropboxListFolderResult {
    entries: Vec<DropboxListEntry>,
}

#[derive(Deserialize)]
struct DropboxListEntry {
    #[serde(rename = ".tag")]
    tag: String,
    id: Option<String>,
    name: Option<String>,
    size: Option<u64>,
    server_modified: Option<String>,
}

impl DropboxBackupProvider {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            client: reqwest::Client::new(),
            folder_ensured: Arc::new(Mutex::new(false)),
        }
    }

    /// Ensure the backup folder exists (cached — only makes the API call once per session).
    async fn ensure_folder(&self, token: &str) -> Result<(), BoxError> {
        {
            let ensured = self.folder_ensured.lock().await;
            if *ensured {
                return Ok(());
            }
        }

        let resp = self
            .client
            .post("https://api.dropboxapi.com/2/files/create_folder_v2")
            .bearer_auth(token)
            .header("Content-Type", "application/json")
            .body(serde_json::json!({ "path": BACKUP_FOLDER, "autorename": false }).to_string())
            .send()
            .await?;

        // 409 = folder already exists — that's expected
        if resp.status() == 409 || resp.status().is_success() {
            let mut ensured = self.folder_ensured.lock().await;
            *ensured = true;
            return Ok(());
        }

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(format!("Failed to create Dropbox folder ({}): {}", status, body).into())
    }
}

#[async_trait]
impl BackupProvider for DropboxBackupProvider {
    fn name(&self) -> &str {
        "Dropbox"
    }

    fn id(&self) -> &str {
        "dropbox"
    }

    fn oauth_provider(&self) -> &str {
        "dropbox"
    }

    async fn verify_access(&self) -> Result<(), BoxError> {
        let token = super::get_oauth_token(&self.pool, "dropbox").await?;
        self.ensure_folder(&token).await?;
        Ok(())
    }

    async fn upload(
        &self,
        file_path: &Path,
        filename: &str,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<String, BoxError> {
        let token = super::get_oauth_token(&self.pool, "dropbox").await?;
        self.ensure_folder(&token).await?;

        let file_bytes = tokio::fs::read(file_path).await?;
        let total = file_bytes.len() as u64;
        progress(0, total);
        let dropbox_path = format!("{}/{}", BACKUP_FOLDER, filename);

        let api_arg = serde_json::json!({
            "path": dropbox_path,
            "mode": "add",
            "autorename": true,
            "mute": true,
        });

        let resp = self
            .client
            .post("https://content.dropboxapi.com/2/files/upload")
            .bearer_auth(&token)
            .header("Content-Type", "application/octet-stream")
            .header("Dropbox-API-Arg", api_arg.to_string())
            .body(file_bytes)
            .send()
            .await?
            .error_for_status()?;

        progress(total, total);
        let meta: DropboxFileMetadata = resp.json().await?;
        Ok(meta.id)
    }

    async fn list_backups(&self) -> Result<Vec<BackupEntry>, BoxError> {
        let token = super::get_oauth_token(&self.pool, "dropbox").await?;
        self.ensure_folder(&token).await?;

        let resp = self
            .client
            .post("https://api.dropboxapi.com/2/files/list_folder")
            .bearer_auth(&token)
            .header("Content-Type", "application/json")
            .body(serde_json::json!({ "path": BACKUP_FOLDER, "limit": 100 }).to_string())
            .send()
            .await?
            .error_for_status()?;

        let result: DropboxListFolderResult = resp.json().await?;

        let mut entries = Vec::new();
        for entry in result.entries {
            if entry.tag != "file" {
                continue;
            }
            let created_at = entry
                .server_modified
                .as_deref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(Utc::now);

            entries.push(BackupEntry {
                id: entry.id.unwrap_or_default(),
                filename: entry.name.unwrap_or_default(),
                size_bytes: entry.size.unwrap_or(0),
                created_at,
            });
        }

        // Sort newest first
        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        Ok(entries)
    }

    async fn download(
        &self,
        backup_id: &str,
        dest: &Path,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<(), BoxError> {
        let token = super::get_oauth_token(&self.pool, "dropbox").await?;

        let api_arg = serde_json::json!({ "path": backup_id });

        let resp = self
            .client
            .post("https://content.dropboxapi.com/2/files/download")
            .bearer_auth(&token)
            .header("Dropbox-API-Arg", api_arg.to_string())
            .send()
            .await?
            .error_for_status()?;

        let total = resp.content_length().unwrap_or(0);
        let mut file = tokio::fs::File::create(dest).await?;
        let mut downloaded: u64 = 0;
        progress(0, total);

        let mut stream = resp.bytes_stream();
        use futures::StreamExt;
        use tokio::io::AsyncWriteExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;
            progress(downloaded, total);
        }
        file.flush().await?;

        Ok(())
    }

    async fn delete(&self, backup_id: &str) -> Result<(), BoxError> {
        let token = super::get_oauth_token(&self.pool, "dropbox").await?;

        self.client
            .post("https://api.dropboxapi.com/2/files/delete_v2")
            .bearer_auth(&token)
            .header("Content-Type", "application/json")
            .body(serde_json::json!({ "path": backup_id }).to_string())
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }
}
