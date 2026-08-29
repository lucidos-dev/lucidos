use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

/// Full email account row including password
#[derive(Clone, sqlx::FromRow)]
pub struct EmailAccount {
    pub id: Uuid,
    pub name: String,
    pub email_address: String,
    pub imap_host: String,
    pub imap_port: i32,
    pub smtp_host: String,
    pub smtp_port: i32,
    pub username: String,
    pub password: String,
    pub use_tls: bool,
    pub require_send_confirmation: bool,
    pub oauth_account_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// Manual `Debug` so an account never leaks its password through `{:?}` in a log
// line or error message. Every other field (including the username) stays
// visible so the struct is still useful for debugging connection problems.
impl std::fmt::Debug for EmailAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmailAccount")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("email_address", &self.email_address)
            .field("imap_host", &self.imap_host)
            .field("imap_port", &self.imap_port)
            .field("smtp_host", &self.smtp_host)
            .field("smtp_port", &self.smtp_port)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("use_tls", &self.use_tls)
            .field("require_send_confirmation", &self.require_send_confirmation)
            .field("oauth_account_id", &self.oauth_account_id)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

/// Email account info without the password (safe for API responses)
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct EmailAccountInfo {
    pub id: Uuid,
    pub name: String,
    pub email_address: String,
    pub imap_host: String,
    pub imap_port: i32,
    pub smtp_host: String,
    pub smtp_port: i32,
    pub username: String,
    pub use_tls: bool,
    pub require_send_confirmation: bool,
    pub oauth_account_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Store for managing email accounts in the database
pub struct EmailStore;

impl EmailStore {
    /// Insert or update an email account (upsert by name)
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert(
        pool: &PgPool,
        name: &str,
        email_address: &str,
        imap_host: &str,
        imap_port: i32,
        smtp_host: &str,
        smtp_port: i32,
        username: &str,
        use_tls: bool,
        require_send_confirmation: bool,
    ) -> Result<Uuid, sqlx::Error> {
        let result = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO email_accounts (name, email_address, imap_host, imap_port, smtp_host, smtp_port, username, use_tls, require_send_confirmation)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (name) DO UPDATE SET
                email_address = EXCLUDED.email_address,
                imap_host = EXCLUDED.imap_host,
                imap_port = EXCLUDED.imap_port,
                smtp_host = EXCLUDED.smtp_host,
                smtp_port = EXCLUDED.smtp_port,
                username = EXCLUDED.username,
                use_tls = EXCLUDED.use_tls,
                require_send_confirmation = EXCLUDED.require_send_confirmation,
                updated_at = NOW()
            RETURNING id
            "#,
        )
        .bind(name)
        .bind(email_address)
        .bind(imap_host)
        .bind(imap_port)
        .bind(smtp_host)
        .bind(smtp_port)
        .bind(username)
        .bind(use_tls)
        .bind(require_send_confirmation)
        .fetch_one(pool)
        .await?;

        Ok(result)
    }

    /// The `email_accounts.name` an `email_password` credential belongs to.
    ///
    /// Normally the identity function: the credential's `service_name` IS the
    /// account name, since `auth_type` carries what the old `email:` prefix used
    /// to say.
    ///
    /// **Temporary measure**, registered in `docs/temporary-measures.md` under
    /// "`email:`-prefixed credential fallback in `get_email_password`". The strip
    /// is for the rows
    /// `20260805134838_drop_credential_name_prefixes_use_auth_type.sql` had to
    /// leave prefixed, because their bare name was already held by another
    /// non-oauth credential. Without it, editing such a credential resolves no
    /// `email_accounts` row: the settings load 404s, and worse, the password
    /// write silently affects zero rows and reports success while IMAP keeps the
    /// old secret. Remove alongside the `get_email_password` fallback.
    pub fn account_name_for_credential(service_name: &str) -> &str {
        service_name.strip_prefix("email:").unwrap_or(service_name)
    }

    /// Update just the password for an existing email account
    pub async fn update_password(
        pool: &PgPool,
        name: &str,
        password: &str,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE email_accounts
            SET password = $2, updated_at = NOW()
            WHERE name = $1
            "#,
        )
        .bind(name)
        .bind(password)
        .execute(pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Get an email account by name (includes the password)
    pub async fn get(pool: &PgPool, name: &str) -> Result<Option<EmailAccount>, sqlx::Error> {
        sqlx::query_as::<_, EmailAccount>(
            r#"
            SELECT id, name, email_address, imap_host, imap_port, smtp_host, smtp_port, username, password, use_tls, require_send_confirmation, oauth_account_id, created_at, updated_at
            FROM email_accounts
            WHERE name = $1
            "#,
        )
        .bind(name)
        .fetch_optional(pool)
        .await
    }

    /// Get an email account's settings by name, without the password
    /// (safe for the settings UI to pre-fill an edit form).
    pub async fn get_info(
        pool: &PgPool,
        name: &str,
    ) -> Result<Option<EmailAccountInfo>, sqlx::Error> {
        sqlx::query_as::<_, EmailAccountInfo>(
            r#"
            SELECT id, name, email_address, imap_host, imap_port, smtp_host, smtp_port, username, use_tls, require_send_confirmation, oauth_account_id, created_at, updated_at
            FROM email_accounts
            WHERE name = $1
            "#,
        )
        .bind(name)
        .fetch_optional(pool)
        .await
    }

    /// Get the default email account (first by created_at)
    pub async fn get_default(pool: &PgPool) -> Result<Option<EmailAccount>, sqlx::Error> {
        sqlx::query_as::<_, EmailAccount>(
            r#"
            SELECT id, name, email_address, imap_host, imap_port, smtp_host, smtp_port, username, password, use_tls, require_send_confirmation, oauth_account_id, created_at, updated_at
            FROM email_accounts
            ORDER BY created_at ASC
            LIMIT 1
            "#,
        )
        .fetch_optional(pool)
        .await
    }

    /// List all email accounts (without passwords)
    pub async fn list(pool: &PgPool) -> Result<Vec<EmailAccountInfo>, sqlx::Error> {
        sqlx::query_as::<_, EmailAccountInfo>(
            r#"
            SELECT id, name, email_address, imap_host, imap_port, smtp_host, smtp_port, username, use_tls, require_send_confirmation, oauth_account_id, created_at, updated_at
            FROM email_accounts
            ORDER BY name ASC
            "#,
        )
        .fetch_all(pool)
        .await
    }

    /// Link an email account to an OAuth account for XOAUTH2 authentication
    pub async fn link_oauth(
        pool: &PgPool,
        name: &str,
        oauth_account_id: Option<Uuid>,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE email_accounts
            SET oauth_account_id = $2, updated_at = NOW()
            WHERE name = $1
            "#,
        )
        .bind(name)
        .bind(oauth_account_id)
        .execute(pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }
}

// ---------------------------------------------------------------------------
// EmailClient — IMAP read + SMTP send
// ---------------------------------------------------------------------------

/// Summary of an email (for listing)
#[derive(Debug, Clone, Serialize)]
pub struct EmailSummary {
    pub uid: u32,
    pub message_id: String,
    pub from: String,
    pub subject: String,
    pub date: String,
    pub preview: String,
}

/// Metadata about an email attachment (no binary data)
#[derive(Debug, Clone, Serialize)]
pub struct EmailAttachmentInfo {
    pub index: usize,
    pub filename: String,
    pub mime_type: String,
    pub size: usize,
}

/// Full email message
#[derive(Debug, Clone, Serialize)]
pub struct EmailMessage {
    pub uid: u32,
    pub message_id: String,
    pub from: String,
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub date: String,
    pub body: String,
    pub attachments: Vec<EmailAttachmentInfo>,
}

/// A file attachment to include in an email
pub struct EmailAttachment {
    pub filename: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

/// A validated attachment path (path checked, filename extracted) but file not yet read.
///
/// `Debug` carries no secret: both fields are the path the caller supplied, and
/// the refusals below already quote it back. The file CONTENTS live on
/// `EmailAttachment`, which deliberately derives nothing.
#[derive(Debug)]
pub struct ValidatedAttachment {
    pub rel_path: String,
    pub filename: String,
}

impl EmailAttachment {
    /// Validate attachment paths without reading file data.
    /// Returns validated paths + basenames for use in confirmation previews.
    ///
    /// A traversal check alone is not enough here. The path joins onto the
    /// workspace `data/` dir, and the file is mailed to whatever address the
    /// caller named. So `.env` needs no `..` to leave the machine. Every
    /// attachment must name a typed subdirectory from
    /// [`super::KNOWN_DATA_PREFIXES`], the rule the file tools already apply.
    pub fn validate_paths(paths: &[String]) -> Result<Vec<ValidatedAttachment>, String> {
        let mut validated = Vec::new();
        for rel_path in paths {
            if super::is_path_traversal(rel_path) {
                return Err(format!("Invalid attachment path '{}'. Paths must be relative to data/ with no '..' components.", rel_path));
            }
            // Callers pass either spelling, so strip a leading `data/` before
            // the prefix test rather than refusing the fuller one.
            let stripped = rel_path.strip_prefix("data/").unwrap_or(rel_path);
            if !super::is_known_data_prefix(stripped) {
                return Err(format!(
                    "Invalid attachment path '{rel_path}'. An attachment must sit under one of \
                     data/'s typed subdirectories ({}), not the data/ root, which holds \
                     gitignored config.",
                    super::KNOWN_DATA_PREFIXES.join(", ")
                ));
            }
            let filename = std::path::Path::new(stripped)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            validated.push(ValidatedAttachment {
                rel_path: stripped.to_string(),
                filename,
            });
        }
        Ok(validated)
    }

    /// Read attachments from workspace data/ directory.
    /// Validates paths and reads file data in one step.
    pub fn read_from_workspace(
        workspace_path: &std::path::Path,
        relative_paths: &[String],
    ) -> Result<Vec<Self>, String> {
        let validated = Self::validate_paths(relative_paths)?;
        let mut attachments = Vec::new();
        for v in validated {
            let full_path = workspace_path.join("data").join(&v.rel_path);
            let data = std::fs::read(&full_path)
                .map_err(|e| format!("Failed to read attachment '{}': {}", v.rel_path, e))?;
            let mime_type = mime_type_from_extension(&v.filename);
            attachments.push(EmailAttachment {
                filename: v.filename,
                mime_type,
                data,
            });
        }
        Ok(attachments)
    }
}

/// Detect MIME type from file extension
fn mime_type_from_extension(filename: &str) -> String {
    let ext = filename
        .rsplit_once('.')
        .map(|(_, e)| e)
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "text/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "csv" => "text/csv",
        "txt" | "md" | "log" => "text/plain",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" | "gzip" => "application/gzip",
        "tar" => "application/x-tar",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Client for reading and sending emails via IMAP/SMTP
pub struct EmailClient;

/// Reject a raw IMAP SEARCH fragment that carries a bare CR or LF.
///
/// [`build_imap_query`]'s output is interpolated verbatim into the
/// `UID SEARCH <query>` command line by `async_imap::Session::uid_search`,
/// which (unlike `select` / `create` / `rename`) does NOT run the crate's own
/// `validate_str` guard. A line break in `search` or `since` therefore ends the
/// command line, and everything after it is read by the server as a SECOND
/// command: a `read_emails` argument of
/// `ALL<CR><LF>A1 STORE 1:* +FLAGS (\Deleted)` flags the whole mailbox for
/// deletion. Both arguments come straight off the `read_emails` tool call, and
/// that tool's own results put untrusted message bodies into the model's
/// context, so the injection is reachable without a compromised user.
///
/// Neither character can appear in a legitimate criterion (RFC 3501 makes the
/// line terminator the command delimiter), so rejecting them costs no real
/// search. Mirrors async-imap's `validate_str`, which checks exactly these two.
fn reject_imap_line_break(label: &str, value: &str) -> Result<(), BoxError> {
    if value.contains(['\r', '\n']) {
        return Err(format!(
            "email {label} must not contain a line break: it is sent verbatim on the IMAP \
             command line, where a carriage return or newline would run as a second command"
        )
        .into());
    }
    Ok(())
}

/// Build an IMAP SEARCH query string from optional search criteria and date filter.
/// The `search` parameter is passed through directly as IMAP criteria (e.g. `FROM "user@example.com"`).
/// Callers MUST have run [`reject_imap_line_break`] over both arguments first.
fn build_imap_query(search: Option<&str>, since: Option<&str>) -> String {
    match (search, since) {
        (Some(s), Some(d)) => format!("SINCE {} {}", d, s),
        (Some(s), None) => s.to_string(),
        (None, Some(d)) => format!("SINCE {}", d),
        (None, None) => "ALL".to_string(),
    }
}

/// Strip non-ASCII characters from quoted strings within an IMAP search query.
/// Leaves keywords and structure intact, only sanitizes the user-visible search terms.
/// Returns the sanitized query suitable for US-ASCII-only IMAP servers.
fn sanitize_search_query(query: &str) -> String {
    let mut result = String::with_capacity(query.len());
    let mut in_quotes = false;
    for ch in query.chars() {
        if ch == '"' {
            in_quotes = !in_quotes;
            result.push(ch);
        } else if in_quotes {
            if ch.is_ascii() {
                result.push(ch);
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// Check whether an email matches the search criteria from the original (possibly non-ASCII) query.
/// Parses common IMAP search criteria (FROM, SUBJECT, TO, CC, TEXT) and checks case-insensitively.
/// Server-side-only criteria (UNSEEN, SEEN, FLAGGED, etc.) are treated as matching.
fn matches_search_filter(email: &EmailSummary, search: &str) -> bool {
    let criteria = parse_search_criteria(search);
    if criteria.is_empty() {
        return true;
    }
    criteria.iter().all(|c| matches_single(email, c))
}

fn matches_single(email: &EmailSummary, criterion: &SearchCriterion) -> bool {
    match criterion {
        SearchCriterion::From(term) => email.from.to_lowercase().contains(&term.to_lowercase()),
        SearchCriterion::Subject(term) => {
            email.subject.to_lowercase().contains(&term.to_lowercase())
        }
        SearchCriterion::To => true, // EmailSummary lacks recipient fields; trust server-side filter
        SearchCriterion::Text(term) => {
            let term_lower = term.to_lowercase();
            email.from.to_lowercase().contains(&term_lower)
                || email.subject.to_lowercase().contains(&term_lower)
                || email.preview.to_lowercase().contains(&term_lower)
        }
        SearchCriterion::Or(a, b) => matches_single(email, a) || matches_single(email, b),
        SearchCriterion::ServerOnly => true,
    }
}

#[derive(Debug, Clone)]
enum SearchCriterion {
    From(String),
    Subject(String),
    To,
    Text(String),
    Or(Box<SearchCriterion>, Box<SearchCriterion>),
    ServerOnly,
}

/// Parse an IMAP search query into criteria for client-side filtering.
/// Handles: FROM "x", SUBJECT "x", TO "x", TEXT "x", OR <c1> <c2>, and server-only keywords.
fn parse_search_criteria(query: &str) -> Vec<SearchCriterion> {
    let mut criteria = Vec::new();
    let tokens = tokenize_imap_query(query);
    let mut i = 0;
    while i < tokens.len() {
        let token_upper = tokens[i].to_uppercase();
        match token_upper.as_str() {
            "OR" => {
                let mut j = i + 1;
                let a = parse_single_criterion(&tokens, &mut j);
                let b = parse_single_criterion(&tokens, &mut j);
                criteria.push(SearchCriterion::Or(Box::new(a), Box::new(b)));
                i = j;
            }
            _ => {
                criteria.push(parse_single_criterion(&tokens, &mut i));
            }
        }
    }
    criteria
}

/// Parse a single IMAP search criterion starting at `pos`, advancing `pos` past it.
fn parse_single_criterion(tokens: &[String], pos: &mut usize) -> SearchCriterion {
    if *pos >= tokens.len() {
        return SearchCriterion::ServerOnly;
    }
    let token_upper = tokens[*pos].to_uppercase();
    match token_upper.as_str() {
        "FROM" | "SUBJECT" | "TO" | "CC" | "BCC" | "TEXT" | "BODY" => {
            *pos += 1;
            if *pos < tokens.len() {
                let value = unquote(&tokens[*pos]);
                *pos += 1;
                match token_upper.as_str() {
                    "FROM" => SearchCriterion::From(value),
                    "SUBJECT" => SearchCriterion::Subject(value),
                    "TO" | "CC" | "BCC" => SearchCriterion::To,
                    _ => SearchCriterion::Text(value),
                }
            } else {
                SearchCriterion::ServerOnly
            }
        }
        "SINCE" | "BEFORE" | "ON" | "SENTBEFORE" | "SENTSINCE" | "SENTON" => {
            // Date criteria with an argument — skip the date value too
            *pos += 2;
            SearchCriterion::ServerOnly
        }
        _ => {
            *pos += 1;
            SearchCriterion::ServerOnly
        }
    }
}

/// Tokenize an IMAP search query into tokens, keeping quoted strings as single tokens.
fn tokenize_imap_query(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in query.chars() {
        if ch == '"' {
            in_quotes = !in_quotes;
            current.push(ch);
        } else if ch.is_whitespace() && !in_quotes {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Remove surrounding quotes from a string if present.
fn unquote(s: &str) -> String {
    s.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s)
        .to_string()
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Format an `Address` from mail_parser into a human-readable string
fn format_address(addr: Option<&mail_parser::Address<'_>>) -> String {
    let Some(addr) = addr else {
        return String::new();
    };
    let parts: Vec<String> = addr
        .iter()
        .map(|a| {
            let email = a.address().unwrap_or("");
            match a.name() {
                Some(name) => format!("{} <{}>", name, email),
                None => email.to_string(),
            }
        })
        .collect();
    parts.join(", ")
}

/// Extract MIME type string from a mail_parser ContentType.
fn mime_type_from_content_type(ct: Option<&mail_parser::ContentType<'_>>) -> String {
    ct.map(|ct| {
        let ctype = ct.ctype();
        let sub = ct.subtype().unwrap_or("octet-stream");
        format!("{}/{}", ctype, sub)
    })
    .unwrap_or_else(|| "application/octet-stream".to_string())
}

/// Derive a fallback filename from a MIME part's content type.
fn fallback_attachment_name(part: &mail_parser::MessagePart<'_>) -> String {
    use mail_parser::MimeHeaders;
    let ct: Option<&mail_parser::ContentType<'_>> = part.content_type();
    ct.and_then(|ct| match ct.subtype().unwrap_or("bin") {
        "pdf" => Some("attachment.pdf"),
        "png" => Some("attachment.png"),
        "jpeg" => Some("attachment.jpeg"),
        "gif" => Some("attachment.gif"),
        _ => None,
    })
    .unwrap_or("attachment.bin")
    .to_string()
}

/// Extract attachment metadata from a parsed email message.
fn extract_attachment_info(parsed: &mail_parser::Message<'_>) -> Vec<EmailAttachmentInfo> {
    use mail_parser::MimeHeaders;

    let mut attachments = Vec::new();
    let count = parsed.attachment_count();
    for i in 0..count {
        if let Some(part) = parsed.attachment(i) {
            let filename = part
                .attachment_name()
                .map(|s| s.to_string())
                .unwrap_or_else(|| fallback_attachment_name(part));

            let ct: Option<&mail_parser::ContentType<'_>> = part.content_type();
            let mime_type = mime_type_from_content_type(ct);
            let size = part.contents().len();

            attachments.push(EmailAttachmentInfo {
                index: i,
                filename,
                mime_type,
                size,
            });
        }
    }
    attachments
}

/// Strip HTML tags from a string (simple regex-free approach)
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    // Collapse whitespace
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A stream type that can be either plain TCP or TLS-wrapped TCP.
#[derive(Debug)]
enum ImapStream {
    Plain(tokio::net::TcpStream),
    Tls(tokio_native_tls::TlsStream<tokio::net::TcpStream>),
}

impl tokio::io::AsyncRead for ImapStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            ImapStream::Plain(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            ImapStream::Tls(s) => std::pin::Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for ImapStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            ImapStream::Plain(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            ImapStream::Tls(s) => std::pin::Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            ImapStream::Plain(s) => std::pin::Pin::new(s).poll_flush(cx),
            ImapStream::Tls(s) => std::pin::Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            ImapStream::Plain(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            ImapStream::Tls(s) => std::pin::Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// XOAUTH2 authenticator for IMAP SASL authentication.
/// Produces the XOAUTH2 string: `user=<email>\x01auth=Bearer <token>\x01\x01`
struct XOAuth2 {
    user: String,
    access_token: String,
}

impl async_imap::Authenticator for &XOAuth2 {
    type Response = String;

    fn process(&mut self, _challenge: &[u8]) -> Self::Response {
        format!(
            "user={}\x01auth=Bearer {}\x01\x01",
            self.user, self.access_token
        )
    }
}

/// Hard wall-clock bound for establishing one IMAP session (TCP connect +
/// TLS + greeting + auth). None of those phases carries its own timeout —
/// `TcpStream::connect` waits on the OS (a filtered route can take 75s+ on
/// macOS) and the greeting read is unbounded — so this gives the most common
/// failure (an unreachable server) a fast, sharp error. Post-auth stalls are
/// covered by the whole-operation `IMAP_OP_TIMEOUT` in `email_client.rs`;
/// sibling of `SMTP_SEND_TIMEOUT` there.
const IMAP_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Connect to an IMAP server and return an authenticated session.
/// Uses TLS if `account.use_tls` is true, plain TCP otherwise.
/// If `oauth_access_token` is provided, uses XOAUTH2 SASL authentication
/// instead of plain LOGIN.
async fn imap_connect(
    account: &EmailAccount,
    oauth_access_token: Option<&str>,
) -> Result<async_imap::Session<ImapStream>, BoxError> {
    imap_connect_with_timeout(account, oauth_access_token, IMAP_CONNECT_TIMEOUT).await
}

/// `imap_connect` with an explicit wall-clock bound (tests pass a short one).
async fn imap_connect_with_timeout(
    account: &EmailAccount,
    oauth_access_token: Option<&str>,
    connect_timeout: std::time::Duration,
) -> Result<async_imap::Session<ImapStream>, BoxError> {
    tokio::time::timeout(
        connect_timeout,
        imap_connect_unbounded(account, oauth_access_token),
    )
    .await
    .map_err(|_| {
        format!(
            "IMAP connect to {}:{} timed out after {}s — the server did not respond \
             (network may block IMAP, or the server is stalled)",
            account.imap_host,
            account.imap_port,
            connect_timeout.as_secs()
        )
    })?
}

async fn imap_connect_unbounded(
    account: &EmailAccount,
    oauth_access_token: Option<&str>,
) -> Result<async_imap::Session<ImapStream>, BoxError> {
    let addr = (account.imap_host.as_str(), account.imap_port as u16);
    let tcp = tokio::net::TcpStream::connect(addr).await?;

    let stream = if account.use_tls {
        let connector = native_tls::TlsConnector::new()?;
        let connector = tokio_native_tls::TlsConnector::from(connector);
        let tls_stream = connector.connect(&account.imap_host, tcp).await?;
        ImapStream::Tls(tls_stream)
    } else {
        ImapStream::Plain(tcp)
    };

    let mut client = async_imap::Client::new(stream);
    client
        .read_response()
        .await
        .ok_or("No greeting from IMAP server")??;

    let session = if let Some(token) = oauth_access_token {
        let auth = XOAuth2 {
            user: account.username.clone(),
            access_token: token.to_string(),
        };
        client
            .authenticate("XOAUTH2", &auth)
            .await
            .map_err(|(err, _)| err)?
    } else {
        client
            .login(&account.username, &account.password)
            .await
            .map_err(|(err, _)| err)?
    };

    Ok(session)
}

#[path = "email_client.rs"]
mod email_client;

#[cfg(test)]
#[path = "email_tests.rs"]
mod tests;
