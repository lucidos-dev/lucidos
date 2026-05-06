use super::{BackupEntry, BackupProvider, BoxError};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::PgPool;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

const FOLDER_NAME: &str = "Lucidos Backups";
const DRIVE_FILES_URL: &str = "https://www.googleapis.com/drive/v3/files";
const DRIVE_UPLOAD_URL: &str = "https://www.googleapis.com/upload/drive/v3/files";

pub struct GoogleDriveBackupProvider {
    pool: PgPool,
    client: reqwest::Client,
    folder_id_cache: Arc<Mutex<Option<String>>>,
}

#[derive(Deserialize)]
struct DriveFile {
    id: String,
    name: Option<String>,
    size: Option<String>,
    #[serde(rename = "createdTime")]
    created_time: Option<String>,
}

#[derive(Deserialize)]
struct DriveFileList {
    files: Vec<DriveFile>,
}

#[derive(Deserialize)]
struct DriveCreateResponse {
    id: String,
}

/// Check a Google Drive API response, returning actionable errors for auth failures
/// and the generic reqwest error for everything else.
fn check_drive_status(resp: reqwest::Response) -> Result<reqwest::Response, BoxError> {
    match resp.status() {
        s if s.is_success() => Ok(resp),
        reqwest::StatusCode::FORBIDDEN => {
            Err("Google Drive access denied (403). Go to Settings > Backup and click 'Grant access' to authorize Drive permissions.".into())
        }
        reqwest::StatusCode::UNAUTHORIZED => {
            Err("Google Drive authentication failed (401). Go to Settings > Backup and click 'Grant access' to re-authorize.".into())
        }
        _ => Ok(resp.error_for_status()?),
    }
}

impl GoogleDriveBackupProvider {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            client: reqwest::Client::new(),
            folder_id_cache: Arc::new(Mutex::new(None)),
        }
    }

    async fn get_token(&self) -> Result<String, BoxError> {
        super::get_oauth_token(&self.pool, "google").await
    }

    async fn get_or_create_folder(&self, token: &str) -> Result<String, BoxError> {
        // Check cache first
        {
            let cache = self.folder_id_cache.lock().await;
            if let Some(ref id) = *cache {
                return Ok(id.clone());
            }
        }

        // Search for existing folder
        let query = format!(
            "name = '{}' and mimeType = 'application/vnd.google-apps.folder' and trashed = false",
            FOLDER_NAME
        );
        let resp = self
            .client
            .get(DRIVE_FILES_URL)
            .bearer_auth(token)
            .query(&[("q", &query), ("fields", &"files(id)".to_string())])
            .send()
            .await?;
        let resp = check_drive_status(resp)?;

        let list: DriveFileList = resp.json().await?;
        let folder_id = if let Some(folder) = list.files.into_iter().next() {
            folder.id
        } else {
            // Create the folder
            let metadata = serde_json::json!({
                "name": FOLDER_NAME,
                "mimeType": "application/vnd.google-apps.folder",
            });
            let resp = check_drive_status(
                self.client
                    .post(DRIVE_FILES_URL)
                    .bearer_auth(token)
                    .json(&metadata)
                    .send()
                    .await?,
            )?;

            let created: DriveCreateResponse = resp.json().await?;
            created.id
        };

        // Cache the folder ID
        {
            let mut cache = self.folder_id_cache.lock().await;
            *cache = Some(folder_id.clone());
        }

        Ok(folder_id)
    }
}

#[async_trait]
impl BackupProvider for GoogleDriveBackupProvider {
    fn name(&self) -> &str {
        "Google Drive"
    }

    fn id(&self) -> &str {
        "google_drive"
    }

    fn oauth_provider(&self) -> &str {
        "google"
    }

    async fn verify_access(&self) -> Result<(), BoxError> {
        let token = self.get_token().await?;
        self.get_or_create_folder(&token).await?;
        Ok(())
    }

    async fn upload(
        &self,
        file_path: &Path,
        filename: &str,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<String, BoxError> {
        let token = self.get_token().await?;
        let folder_id = self.get_or_create_folder(&token).await?;

        let file_bytes = tokio::fs::read(file_path).await?;
        let total = file_bytes.len() as u64;
        progress(0, total);

        let metadata = serde_json::json!({
            "name": filename,
            "parents": [folder_id],
        });

        let metadata_part =
            reqwest::multipart::Part::text(metadata.to_string()).mime_str("application/json")?;
        let file_part =
            reqwest::multipart::Part::bytes(file_bytes).mime_str("application/octet-stream")?;

        let form = reqwest::multipart::Form::new()
            .part("metadata", metadata_part)
            .part("file", file_part);

        let url = format!("{}?uploadType=multipart&fields=id", DRIVE_UPLOAD_URL);
        let resp = check_drive_status(
            self.client
                .post(&url)
                .bearer_auth(&token)
                .multipart(form)
                .send()
                .await?,
        )?;

        progress(total, total);
        let created: DriveCreateResponse = resp.json().await?;
        Ok(created.id)
    }

    async fn list_backups(&self) -> Result<Vec<BackupEntry>, BoxError> {
        let token = self.get_token().await?;
        let folder_id = self.get_or_create_folder(&token).await?;

        let query = format!("'{}' in parents and trashed = false", folder_id);
        let resp = check_drive_status(
            self.client
                .get(DRIVE_FILES_URL)
                .bearer_auth(&token)
                .query(&[
                    ("q", query.as_str()),
                    ("fields", "files(id,name,size,createdTime)"),
                    ("orderBy", "createdTime desc"),
                    ("pageSize", "100"),
                ])
                .send()
                .await?,
        )?;

        let list: DriveFileList = resp.json().await?;

        let mut entries = Vec::with_capacity(list.files.len());
        for file in list.files {
            let size_bytes = file
                .size
                .as_deref()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);

            let created_at = file
                .created_time
                .as_deref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(Utc::now);

            entries.push(BackupEntry {
                id: file.id,
                filename: file.name.unwrap_or_default(),
                size_bytes,
                created_at,
            });
        }

        Ok(entries)
    }

    async fn download(
        &self,
        backup_id: &str,
        dest: &Path,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<(), BoxError> {
        let token = self.get_token().await?;

        let url = format!("{}/{}?alt=media", DRIVE_FILES_URL, backup_id);
        let resp = check_drive_status(self.client.get(&url).bearer_auth(&token).send().await?)?;

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
        let token = self.get_token().await?;

        let url = format!("{}/{}", DRIVE_FILES_URL, backup_id);
        check_drive_status(self.client.delete(&url).bearer_auth(&token).send().await?)?;

        Ok(())
    }
}
