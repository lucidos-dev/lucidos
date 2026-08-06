use super::{BackupEntry, BackupProvider, BoxError};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::PgPool;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

const BACKUP_FOLDER: &str = "/Lucidos Backups";

/// Every Dropbox scope a working backup needs, and why:
///
/// - `files.content.write`: `create_folder_v2`, `files/upload`, `files/delete_v2`
/// - `files.content.read`: `files/download`, i.e. restoring an archive
/// - `files.metadata.read`: `files/list_folder`, which drives retention pruning
///   and the "last cloud backup" health card
///
/// This IS the provider registry's `required_scopes` entry, so the readiness
/// verdict the Settings page renders and the preflight below check one list.
/// Two lists would let an account holding only the first scope read as ready,
/// hiding *Grant access* behind a *Back up now* that fails at preflight.
///
/// `account_info.read` is deliberately NOT here. It only names the connected
/// account in Settings, so its absence costs an email address, never a backup,
/// and requiring it would make an otherwise working account read as broken. The
/// request set in `backupProviderScopes.ts` asks for it anyway.
pub const BACKUP_SCOPES: &[&str] = &[
    "files.content.write",
    "files.content.read",
    "files.metadata.read",
];

/// The scopes a user actually grants. Dropbox's requirements ARE whole scope
/// names, so this is the same list plus `account_info.read`, which is requested
/// but not required (see above). Exists so every provider answers
/// [`super::name_missing_scopes`] the same way, including the one whose
/// requirements are substrings.
///
/// Mirrors the request string in `backupProviderScopes.ts`, which is what the
/// *Grant access* button actually sends. Change one and change the other: a
/// scope requested but absent here would be named by its raw matcher instead.
pub const GRANT_SCOPES: &[&str] = &[
    "files.content.write",
    "files.content.read",
    "files.metadata.read",
    "account_info.read",
];

/// Pull the scope name out of a Dropbox permission error, whichever shape it
/// arrives in. Dropbox reports the same condition two ways: a structured
/// `{"error": {".tag": "missing_scope", "required_scope": "..."}}` body, and a
/// prose 400 naming the scope in quotes (the shape a user reported on
/// 2026-08-05: *does not have the required scope 'files.content.write'*).
/// `None` when the body is about something else entirely.
pub fn missing_scope_from_body(body: &str) -> Option<String> {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(scope) = json["error"]["required_scope"].as_str() {
            return Some(scope.to_string());
        }
    }
    // Prose form. Take what sits between the first pair of single quotes after
    // the phrase, rather than scanning the whole body, so an unrelated quoted
    // string elsewhere in the message can't be reported as a scope name.
    let after = body.split_once("required scope")?.1;
    let inner = after.split_once('\'')?.1;
    let (scope, _) = inner.split_once('\'')?;
    (!scope.is_empty()).then(|| scope.to_string())
}

/// What to tell the user when the connected Dropbox account is short a scope.
///
/// Names all three parts of the fix, because each one alone leaves them stuck:
/// the App Console is where the permission is enabled, and re-connecting is what
/// actually issues a token carrying it. Dropbox never upgrades a token or an
/// existing grant when a permission is ticked in the console.
pub fn missing_scope_message(missing: &[&str]) -> String {
    format!(
        "Dropbox is missing the permission{} {}. Enable {} on the Permissions tab of your app in the Dropbox App Console, then reconnect the account in Settings > Accounts (ticking the box does not change a token that already exists).",
        if missing.len() == 1 { "" } else { "s" },
        missing.join(", "),
        if missing.len() == 1 { "it" } else { "them" },
    )
}

pub struct DropboxBackupProvider {
    pool: PgPool,
    client: reqwest::Client,
    folder_ensured: Arc<Mutex<bool>>,
    /// Scopes the token must carry, threaded from the provider registry's
    /// `required_scopes` so there's a single source of truth for them.
    required_scopes: &'static [&'static str],
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
    pub fn new(pool: PgPool, required_scopes: &'static [&'static str]) -> Self {
        Self {
            pool,
            client: reqwest::Client::new(),
            folder_ensured: Arc::new(Mutex::new(false)),
            required_scopes,
        }
    }

    /// Fail before any real work when the connected account's granted scopes
    /// can't support a backup.
    ///
    /// Dropbox has no `tokeninfo` endpoint to introspect a token with (the way
    /// `google_drive::verify_scope` does), but it does return the granted
    /// `scope` in the token response, and `prepare_oauth_flow` stores exactly
    /// that. So the account row is the authoritative record of what this token
    /// can do, which is why this takes the stored scopes rather than making a
    /// call: no network, and no second account lookup in preflight.
    fn verify_scopes(&self, granted: &str) -> Result<(), BoxError> {
        let missing = super::missing_scopes(granted, self.required_scopes);
        if missing.is_empty() {
            Ok(())
        } else {
            Err(missing_scope_message(&missing).into())
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

        check_dropbox_status(resp, "create the backups folder").await?;
        Ok(())
    }
}

/// Turn a failed Dropbox response into an error the user can act on.
///
/// `context` names the operation ("upload the backup", "list backups") so the
/// message says which leg failed. A permission failure is rewritten into
/// [`missing_scope_message`]; everything else keeps the status and body, which
/// is all Dropbox gives us to go on. Preflight normally catches the scope case
/// first, so reaching this branch means a scope was revoked mid-life or the
/// stored grant over-reports what the token can do.
async fn check_dropbox_status(
    resp: reqwest::Response,
    context: &str,
) -> Result<reqwest::Response, BoxError> {
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if let Some(scope) = missing_scope_from_body(&body) {
        return Err(missing_scope_message(&[scope.as_str()]).into());
    }
    Err(format!("Dropbox failed to {} ({}): {}", context, status, body).into())
}

/// Build the Dropbox web URL for the backups folder. The Dropbox web app
/// addresses folders by path under `/home`; the folder path is fixed, so this is
/// always a real deep link (no id lookup needed). Pure for unit-testability.
fn dropbox_folder_url() -> String {
    let mut url =
        reqwest::Url::parse("https://www.dropbox.com/home").expect("static base URL is valid");
    url.path_segments_mut()
        .expect("base URL is a base")
        .extend(BACKUP_FOLDER.split('/').filter(|s| !s.is_empty()));
    url.to_string()
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

    async fn folder_url(&self) -> Option<String> {
        Some(dropbox_folder_url())
    }

    async fn preflight(&self, _estimated_upload_bytes: u64) -> Result<(), BoxError> {
        // Cheap, ordered checks, same shape as Drive's: fail before any
        // expensive work and keep the permission guidance for an actual
        // permission problem. There is no free-space check because Dropbox
        // exposes no quota endpoint we can preflight against, so the
        // `_estimated_upload_bytes` hint stays unused.
        // 1. Account: connect / refresh. Fetched whole rather than as a bare
        //    token, so step 2 reads the granted scopes off the same row instead
        //    of repeating the lookup.
        let account = super::get_oauth_account(&self.pool, "dropbox").await?;
        // 2. Granted scopes. Before this existed the first Dropbox call a
        //    backup made was the folder create, so a short-scoped token
        //    surfaced as a raw 400 after the archive work had already run.
        self.verify_scopes(&account.scopes)?;
        // 3. Folder: verifies write access and warms the cache for the upload.
        self.ensure_folder(&account.access_token).await?;
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

        let resp = check_dropbox_status(
            self.client
                .post("https://content.dropboxapi.com/2/files/upload")
                .bearer_auth(&token)
                .header("Content-Type", "application/octet-stream")
                .header("Dropbox-API-Arg", api_arg.to_string())
                .body(file_bytes)
                .send()
                .await?,
            "upload the backup",
        )
        .await?;

        progress(total, total);
        let meta: DropboxFileMetadata = resp.json().await?;
        Ok(meta.id)
    }

    async fn list_backups(&self) -> Result<Vec<BackupEntry>, BoxError> {
        let token = super::get_oauth_token(&self.pool, "dropbox").await?;
        self.ensure_folder(&token).await?;

        let resp = check_dropbox_status(
            self.client
                .post("https://api.dropboxapi.com/2/files/list_folder")
                .bearer_auth(&token)
                .header("Content-Type", "application/json")
                .body(serde_json::json!({ "path": BACKUP_FOLDER, "limit": 100 }).to_string())
                .send()
                .await?,
            "list backups",
        )
        .await?;

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

        let resp = check_dropbox_status(
            self.client
                .post("https://content.dropboxapi.com/2/files/download")
                .bearer_auth(&token)
                .header("Dropbox-API-Arg", api_arg.to_string())
                .send()
                .await?,
            "download the backup",
        )
        .await?;

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

        check_dropbox_status(
            self.client
                .post("https://api.dropboxapi.com/2/files/delete_v2")
                .bearer_auth(&token)
                .header("Content-Type", "application/json")
                .body(serde_json::json!({ "path": backup_id }).to_string())
                .send()
                .await?,
            "delete an old backup",
        )
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropbox_folder_url_deep_links_to_home_path() {
        // The space in "Lucidos Backups" must be percent-encoded, never raw.
        let url = dropbox_folder_url();
        assert!(url.starts_with("https://www.dropbox.com/home/"));
        assert!(url.contains("Lucidos"));
        assert!(!url.contains(' '));
        assert_eq!(url, "https://www.dropbox.com/home/Lucidos%20Backups");
    }

    #[test]
    fn a_fully_scoped_grant_is_missing_nothing() {
        let granted =
            "account_info.read files.metadata.read files.content.read files.content.write";
        assert!(super::super::missing_scopes(granted, BACKUP_SCOPES).is_empty());
    }

    /// The reported failure: an account connected for its name only.
    #[test]
    fn an_account_info_only_grant_is_missing_every_backup_scope() {
        assert_eq!(
            super::super::missing_scopes("account_info.read", BACKUP_SCOPES),
            vec![
                "files.content.write",
                "files.content.read",
                "files.metadata.read"
            ]
        );
    }

    /// The case that made readiness and preflight disagree while they held
    /// separate lists: a token that CAN write but cannot list. Readiness reads
    /// the same list now, so the page says not-ready and offers Grant access
    /// rather than enabling a backup that fails at preflight.
    #[test]
    fn a_write_only_grant_is_still_missing_the_read_scopes() {
        assert_eq!(
            super::super::missing_scopes("files.content.write", BACKUP_SCOPES),
            vec!["files.content.read", "files.metadata.read"]
        );
    }

    #[test]
    fn one_missing_scope_is_reported_alone() {
        let granted = "files.content.write files.content.read account_info.read";
        assert_eq!(
            super::super::missing_scopes(granted, BACKUP_SCOPES),
            vec!["files.metadata.read"]
        );
    }

    #[test]
    fn an_empty_grant_is_missing_every_backup_scope() {
        assert_eq!(
            super::super::missing_scopes("", BACKUP_SCOPES).len(),
            BACKUP_SCOPES.len()
        );
    }

    /// `account_info.read` names the account and nothing else, so an otherwise
    /// complete grant without it must NOT read as broken.
    #[test]
    fn a_grant_without_account_info_is_still_a_working_backup() {
        let granted = "files.content.write files.content.read files.metadata.read";
        assert!(super::super::missing_scopes(granted, BACKUP_SCOPES).is_empty());
    }

    #[test]
    fn the_message_names_the_console_the_scopes_and_the_reconnect() {
        let msg = missing_scope_message(&["files.content.write"]);
        assert!(msg.contains("files.content.write"));
        assert!(msg.contains("Dropbox App Console"));
        assert!(msg.contains("Settings > Accounts"));
        // Singular wording for one scope, plural for several.
        assert!(msg.contains("the permission files.content.write"));
        let many = missing_scope_message(&["files.content.write", "files.metadata.read"]);
        assert!(many.contains("the permissions files.content.write, files.metadata.read"));
    }

    /// The structured error body Dropbox returns for a scoped app.
    #[test]
    fn missing_scope_is_read_from_the_structured_body() {
        let body = r#"{"error_summary":"missing_scope/.","error":{".tag":"missing_scope","required_scope":"files.metadata.read"}}"#;
        assert_eq!(
            missing_scope_from_body(body).as_deref(),
            Some("files.metadata.read")
        );
    }

    /// The prose 400 a user actually hit on 2026-08-05.
    #[test]
    fn missing_scope_is_read_from_the_prose_body() {
        let body = "Error in call to API function \"files/create_folder_v2\": Your app (ID: 1234567) is not permitted to access this endpoint because it does not have the required scope 'files.content.write'. The owner of the app can enable the scope for the app using the Permissions tab on the App Console.";
        assert_eq!(
            missing_scope_from_body(body).as_deref(),
            Some("files.content.write")
        );
    }

    /// An unrelated failure keeps its own message: quoting something is not the
    /// same as naming a scope, and mislabelling a path error as a permission
    /// problem would send the user to the App Console for nothing.
    #[test]
    fn an_unrelated_error_body_names_no_scope() {
        assert_eq!(missing_scope_from_body("insufficient_space"), None);
        assert_eq!(
            missing_scope_from_body(r#"{"error_summary":"path/not_found/."}"#),
            None
        );
        assert_eq!(
            missing_scope_from_body("could not find 'Lucidos Backups'"),
            None
        );
    }
}
