use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

/// Full email account row including password
#[derive(Debug, Clone)]
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

/// Email account info without the password (safe for API responses)
#[derive(Debug, Clone, Serialize, Deserialize)]
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
        let result = sqlx::query_as::<_, (Uuid, String, String, String, i32, String, i32, String, String, bool, bool, Option<Uuid>, DateTime<Utc>, DateTime<Utc>)>(
            r#"
            SELECT id, name, email_address, imap_host, imap_port, smtp_host, smtp_port, username, password, use_tls, require_send_confirmation, oauth_account_id, created_at, updated_at
            FROM email_accounts
            WHERE name = $1
            "#,
        )
        .bind(name)
        .fetch_optional(pool)
        .await?;

        Ok(result.map(
            |(
                id,
                name,
                email_address,
                imap_host,
                imap_port,
                smtp_host,
                smtp_port,
                username,
                password,
                use_tls,
                require_send_confirmation,
                oauth_account_id,
                created_at,
                updated_at,
            )| {
                EmailAccount {
                    id,
                    name,
                    email_address,
                    imap_host,
                    imap_port,
                    smtp_host,
                    smtp_port,
                    username,
                    password,
                    use_tls,
                    require_send_confirmation,
                    oauth_account_id,
                    created_at,
                    updated_at,
                }
            },
        ))
    }

    /// Get the default email account (first by created_at)
    pub async fn get_default(pool: &PgPool) -> Result<Option<EmailAccount>, sqlx::Error> {
        let result = sqlx::query_as::<_, (Uuid, String, String, String, i32, String, i32, String, String, bool, bool, Option<Uuid>, DateTime<Utc>, DateTime<Utc>)>(
            r#"
            SELECT id, name, email_address, imap_host, imap_port, smtp_host, smtp_port, username, password, use_tls, require_send_confirmation, oauth_account_id, created_at, updated_at
            FROM email_accounts
            ORDER BY created_at ASC
            LIMIT 1
            "#,
        )
        .fetch_optional(pool)
        .await?;

        Ok(result.map(
            |(
                id,
                name,
                email_address,
                imap_host,
                imap_port,
                smtp_host,
                smtp_port,
                username,
                password,
                use_tls,
                require_send_confirmation,
                oauth_account_id,
                created_at,
                updated_at,
            )| {
                EmailAccount {
                    id,
                    name,
                    email_address,
                    imap_host,
                    imap_port,
                    smtp_host,
                    smtp_port,
                    username,
                    password,
                    use_tls,
                    require_send_confirmation,
                    oauth_account_id,
                    created_at,
                    updated_at,
                }
            },
        ))
    }

    /// List all email accounts (without passwords)
    pub async fn list(pool: &PgPool) -> Result<Vec<EmailAccountInfo>, sqlx::Error> {
        let results = sqlx::query_as::<_, (Uuid, String, String, String, i32, String, i32, String, bool, bool, Option<Uuid>, DateTime<Utc>, DateTime<Utc>)>(
            r#"
            SELECT id, name, email_address, imap_host, imap_port, smtp_host, smtp_port, username, use_tls, require_send_confirmation, oauth_account_id, created_at, updated_at
            FROM email_accounts
            ORDER BY name ASC
            "#,
        )
        .fetch_all(pool)
        .await?;

        Ok(results
            .into_iter()
            .map(
                |(
                    id,
                    name,
                    email_address,
                    imap_host,
                    imap_port,
                    smtp_host,
                    smtp_port,
                    username,
                    use_tls,
                    require_send_confirmation,
                    oauth_account_id,
                    created_at,
                    updated_at,
                )| {
                    EmailAccountInfo {
                        id,
                        name,
                        email_address,
                        imap_host,
                        imap_port,
                        smtp_host,
                        smtp_port,
                        username,
                        use_tls,
                        require_send_confirmation,
                        oauth_account_id,
                        created_at,
                        updated_at,
                    }
                },
            )
            .collect())
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
pub struct ValidatedAttachment {
    pub rel_path: String,
    pub filename: String,
}

impl EmailAttachment {
    /// Validate attachment paths without reading file data.
    /// Returns validated paths + basenames for use in confirmation previews.
    pub fn validate_paths(paths: &[String]) -> Result<Vec<ValidatedAttachment>, String> {
        let mut validated = Vec::new();
        for rel_path in paths {
            if rel_path.contains("..") || rel_path.starts_with('/') || rel_path.starts_with('\\') {
                return Err(format!("Invalid attachment path '{}'. Paths must be relative to data/ with no '..' components.", rel_path));
            }
            let filename = std::path::Path::new(rel_path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            validated.push(ValidatedAttachment {
                rel_path: rel_path.clone(),
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
pub fn mime_type_from_extension(filename: &str) -> String {
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

/// Build an IMAP SEARCH query string from optional search criteria and date filter.
/// The `search` parameter is passed through directly as IMAP criteria (e.g. `FROM "user@example.com"`).
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

/// Format a byte count as a human-readable size string.
pub fn format_byte_size(bytes: usize) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} bytes", bytes)
    }
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

/// Connect to an IMAP server and return an authenticated session.
/// Uses TLS if `account.use_tls` is true, plain TCP otherwise.
/// If `oauth_access_token` is provided, uses XOAUTH2 SASL authentication
/// instead of plain LOGIN.
async fn imap_connect(
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
    let _greeting = client
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

impl EmailClient {
    /// Read email summaries from an IMAP folder.
    /// If `oauth_access_token` is provided, uses XOAUTH2 instead of password login.
    pub async fn read_emails(
        account: &EmailAccount,
        folder: Option<&str>,
        limit: Option<u32>,
        search: Option<&str>,
        since: Option<&str>,
        oauth_access_token: Option<&str>,
    ) -> Result<Vec<EmailSummary>, BoxError> {
        let folder = folder.unwrap_or("INBOX");
        let limit = limit.unwrap_or(20);

        let mut session = imap_connect(account, oauth_access_token).await?;
        session.select(folder).await?;

        // Detect non-ASCII in search query (Exchange Online only supports US-ASCII IMAP SEARCH)
        let has_non_ascii = search.is_some_and(|s| !s.is_ascii());
        let imap_search = if has_non_ascii {
            let sanitized = sanitize_search_query(search.unwrap());
            crate::log!(@Email, "Non-ASCII detected in search, sanitized: {:?} → {:?}", search.unwrap(), &sanitized);
            sanitized
        } else {
            search.map(|s| s.to_string()).unwrap_or_default()
        };

        let query = build_imap_query(
            if imap_search.is_empty() {
                None
            } else {
                Some(&imap_search)
            },
            since,
        );

        crate::log!(@Email, "IMAP search: {} in {}", query, folder);

        let uids = session.uid_search(&query).await?;
        let mut uid_list: Vec<u32> = uids.into_iter().collect();
        uid_list.sort_unstable();
        uid_list.reverse(); // newest first

        if uid_list.is_empty() {
            session.logout().await?;
            return Ok(Vec::new());
        }

        // When client-side filtering will narrow results, fetch more candidates
        let fetch_limit = if has_non_ascii {
            (limit * 3).max(60)
        } else {
            limit
        };
        uid_list.truncate(fetch_limit as usize);

        let uid_set = uid_list
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");

        let fetch_stream = session
            .uid_fetch(&uid_set, "(UID BODY.PEEK[HEADER] BODY.PEEK[TEXT]<0.500>)")
            .await?;
        let fetches: Vec<_> = fetch_stream.try_collect().await?;

        let parser = mail_parser::MessageParser::default();
        let mut summaries = Vec::new();

        for fetch in &fetches {
            let uid = match fetch.uid {
                Some(u) => u,
                None => continue,
            };

            // Parse header bytes with mail_parser
            let header_bytes = fetch.header().unwrap_or_default();
            // Combine header + partial text for parsing
            let text_bytes = fetch.text().unwrap_or_default();
            let mut combined = Vec::with_capacity(header_bytes.len() + text_bytes.len());
            combined.extend_from_slice(header_bytes);
            combined.extend_from_slice(text_bytes);

            let parsed = parser.parse(&combined);

            let (message_id, from, subject, date, preview) = match &parsed {
                Some(msg) => {
                    let message_id = msg.message_id().unwrap_or("").to_string();
                    let from = format_address(msg.from());
                    let subject = msg.subject().unwrap_or("(no subject)").to_string();
                    let date = msg.date().map(|d| d.to_rfc3339()).unwrap_or_default();

                    // Build preview from text body or HTML body
                    let preview = msg
                        .body_text(0)
                        .map(|t| t.to_string())
                        .or_else(|| msg.body_html(0).map(|h| strip_html_tags(&h)))
                        .unwrap_or_default();

                    // Truncate preview to 200 chars
                    let preview = if preview.chars().count() > 200 {
                        let mut truncated: String = preview.chars().take(200).collect();
                        truncated.push_str("...");
                        truncated
                    } else {
                        preview
                    };

                    (message_id, from, subject, date, preview)
                }
                None => (
                    String::new(),
                    String::new(),
                    "(unparseable)".to_string(),
                    String::new(),
                    String::new(),
                ),
            };

            summaries.push(EmailSummary {
                uid,
                message_id,
                from,
                subject,
                date,
                preview,
            });
        }

        session.logout().await?;

        // Sort by UID descending (newest first)
        summaries.sort_by(|a, b| b.uid.cmp(&a.uid));

        // When the original search had non-ASCII, the IMAP query was broadened
        // (non-ASCII stripped). Now filter client-side against the original query.
        if has_non_ascii {
            let original_search = search.unwrap();
            let before = summaries.len();
            summaries.retain(|email| matches_search_filter(email, original_search));
            summaries.truncate(limit as usize);
            crate::log!(
                @Email,
                "Non-ASCII client-side filter: {} → {} results",
                before,
                summaries.len()
            );
        }

        Ok(summaries)
    }

    /// Read a single full email by UID.
    /// If `oauth_access_token` is provided, uses XOAUTH2 instead of password login.
    pub async fn read_email(
        account: &EmailAccount,
        uid: u32,
        folder: Option<&str>,
        oauth_access_token: Option<&str>,
    ) -> Result<EmailMessage, BoxError> {
        let folder = folder.unwrap_or("INBOX");

        let mut session = imap_connect(account, oauth_access_token).await?;
        session.select(folder).await?;

        let uid_str = uid.to_string();
        let fetch_stream = session.uid_fetch(&uid_str, "(UID RFC822)").await?;
        let fetches: Vec<_> = fetch_stream.try_collect().await?;

        let fetch = fetches
            .first()
            .ok_or_else(|| format!("No message found with UID {}", uid))?;

        let body_bytes = fetch.body().unwrap_or_default();
        let parser = mail_parser::MessageParser::default();
        let parsed = parser
            .parse(body_bytes)
            .ok_or("Failed to parse email message")?;

        let message_id = parsed.message_id().unwrap_or("").to_string();
        let from = format_address(parsed.from());
        let to = format_address(parsed.to());
        let cc = format_address(parsed.cc());
        let subject = parsed.subject().unwrap_or("(no subject)").to_string();
        let date = parsed.date().map(|d| d.to_rfc3339()).unwrap_or_default();

        // Prefer plain text body, fall back to HTML with tag stripping
        let body = parsed
            .body_text(0)
            .map(|t| t.to_string())
            .or_else(|| parsed.body_html(0).map(|h| strip_html_tags(&h)))
            .unwrap_or_default();

        // Extract attachment metadata
        let attachments = extract_attachment_info(&parsed);

        session.logout().await?;

        Ok(EmailMessage {
            uid,
            message_id,
            from,
            to,
            cc,
            subject,
            date,
            body,
            attachments,
        })
    }

    /// Fetch a specific attachment's binary data by UID and attachment index.
    /// Returns (filename, mime_type, data).
    pub async fn fetch_attachment(
        account: &EmailAccount,
        uid: u32,
        attachment_index: usize,
        folder: Option<&str>,
        oauth_access_token: Option<&str>,
    ) -> Result<(String, String, Vec<u8>), BoxError> {
        use mail_parser::MimeHeaders;

        let folder = folder.unwrap_or("INBOX");

        let mut session = imap_connect(account, oauth_access_token).await?;
        session.select(folder).await?;

        let uid_str = uid.to_string();
        let fetch_stream = session.uid_fetch(&uid_str, "(UID RFC822)").await?;
        let fetches: Vec<_> = fetch_stream.try_collect().await?;

        let fetch = fetches
            .first()
            .ok_or_else(|| format!("No message found with UID {}", uid))?;

        let body_bytes = fetch.body().unwrap_or_default();
        let parser = mail_parser::MessageParser::default();
        let parsed = parser
            .parse(body_bytes)
            .ok_or("Failed to parse email message")?;

        let part = parsed
            .attachment(attachment_index)
            .ok_or_else(|| format!("No attachment at index {}", attachment_index))?;

        let filename = part
            .attachment_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| fallback_attachment_name(part));

        let ct: Option<&mail_parser::ContentType<'_>> = part.content_type();
        let mime_type = mime_type_from_content_type(ct);

        let data = part.contents().to_vec();

        session.logout().await?;

        Ok((filename, mime_type, data))
    }

    /// Send an email via SMTP.
    /// If `oauth_access_token` is provided, uses XOAUTH2 authentication instead of password.
    /// If `attachments` is non-empty, builds a multipart/mixed MIME message.
    #[allow(clippy::too_many_arguments)]
    pub async fn send_email(
        account: &EmailAccount,
        to: &str,
        subject: &str,
        body: &str,
        cc: Option<&str>,
        bcc: Option<&str>,
        in_reply_to: Option<&str>,
        oauth_access_token: Option<&str>,
        attachments: &[EmailAttachment],
    ) -> Result<String, BoxError> {
        use lettre::message::Mailbox;
        use lettre::transport::smtp::authentication::{Credentials, Mechanism};
        use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

        let from_mailbox: Mailbox = account
            .email_address
            .parse()
            .map_err(|e| format!("Invalid from address '{}': {}", account.email_address, e))?;

        let mut builder = lettre::Message::builder()
            .from(from_mailbox)
            .subject(subject);

        // Add To recipients
        for addr in to.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            let mailbox: Mailbox = addr
                .parse()
                .map_err(|e| format!("Invalid to address '{}': {}", addr, e))?;
            builder = builder.to(mailbox);
        }

        // Add CC recipients
        if let Some(cc_str) = cc {
            for addr in cc_str
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
            {
                let mailbox: Mailbox = addr
                    .parse()
                    .map_err(|e| format!("Invalid cc address '{}': {}", addr, e))?;
                builder = builder.cc(mailbox);
            }
        }

        // Add BCC recipients
        if let Some(bcc_str) = bcc {
            for addr in bcc_str
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
            {
                let mailbox: Mailbox = addr
                    .parse()
                    .map_err(|e| format!("Invalid bcc address '{}': {}", addr, e))?;
                builder = builder.bcc(mailbox);
            }
        }

        // Add In-Reply-To header
        if let Some(reply_id) = in_reply_to {
            builder = builder.in_reply_to(reply_id.to_string());
        }

        let text_part = lettre::message::SinglePart::builder()
            .header(
                lettre::message::header::ContentType::parse("text/plain; charset=utf-8").unwrap(),
            )
            .body(body.to_string());

        let message = if attachments.is_empty() {
            builder.singlepart(text_part)?
        } else {
            let mut multipart = lettre::message::MultiPart::mixed().singlepart(text_part);

            for att in attachments {
                let content_type = lettre::message::header::ContentType::parse(&att.mime_type)
                    .or_else(|_| {
                        lettre::message::header::ContentType::parse("application/octet-stream")
                    })
                    .expect("fallback content type should always parse");
                let attachment_part = lettre::message::Attachment::new(att.filename.clone())
                    .body(att.data.clone(), content_type);
                multipart = multipart.singlepart(attachment_part);
            }

            builder.multipart(multipart)?
        };

        // Extract Message-ID from the built message
        let message_id = message
            .headers()
            .get_raw("Message-ID")
            .map(|v| v.to_string())
            .unwrap_or_default();

        // Use XOAUTH2 with access token, or fall back to password-based auth
        let (creds, auth_mechanisms) = if let Some(token) = oauth_access_token {
            (
                Credentials::new(account.username.clone(), token.to_string()),
                Some(vec![Mechanism::Xoauth2]),
            )
        } else {
            (
                Credentials::new(account.username.clone(), account.password.clone()),
                None,
            )
        };

        let transport = if account.smtp_port == 465 {
            // Implicit TLS (port 465)
            let mut tb = AsyncSmtpTransport::<Tokio1Executor>::relay(&account.smtp_host)?
                .port(account.smtp_port as u16)
                .credentials(creds);
            if let Some(mechs) = auth_mechanisms {
                tb = tb.authentication(mechs);
            }
            tb.build()
        } else if account.use_tls {
            // STARTTLS (port 587)
            let mut tb = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&account.smtp_host)?
                .port(account.smtp_port as u16)
                .credentials(creds);
            if let Some(mechs) = auth_mechanisms {
                tb = tb.authentication(mechs);
            }
            tb.build()
        } else {
            // No TLS
            let mut tb =
                AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&account.smtp_host)
                    .port(account.smtp_port as u16)
                    .credentials(creds);
            if let Some(mechs) = auth_mechanisms {
                tb = tb.authentication(mechs);
            }
            tb.build()
        };

        transport.send(message).await?;

        let auth_method = if oauth_access_token.is_some() {
            "XOAUTH2"
        } else {
            "password"
        };
        crate::log!(@Email, "Email sent to {} via {} ({})", to, account.smtp_host, auth_method);

        Ok(message_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an email message the same way send_email does, and verify
    /// Content-Type includes charset=utf-8 so emoji/non-ASCII render correctly.
    #[test]
    fn test_xoauth2_authenticator_format() {
        use async_imap::Authenticator;

        let auth = super::XOAuth2 {
            user: "user@outlook.com".to_string(),
            access_token: "ya29.test-token-123".to_string(),
        };
        let response = (&auth).process(b"");

        // XOAUTH2 format: user=<email>\x01auth=Bearer <token>\x01\x01
        let expected = "user=user@outlook.com\x01auth=Bearer ya29.test-token-123\x01\x01";
        assert_eq!(response, expected);
    }

    #[test]
    fn test_email_body_has_utf8_charset() {
        let body = "Hello! 🎉 Congratulations on your achievement — well done!";
        let message = lettre::Message::builder()
            .from("sender@example.com".parse().unwrap())
            .to("recipient@example.com".parse().unwrap())
            .subject("Test")
            .singlepart(
                lettre::message::SinglePart::builder()
                    .header(
                        lettre::message::header::ContentType::parse("text/plain; charset=utf-8")
                            .unwrap(),
                    )
                    .body(body.to_string()),
            )
            .unwrap();

        let raw = String::from_utf8(message.formatted()).unwrap();
        // Verify the raw email contains the UTF-8 charset declaration
        let raw_lower = raw.to_lowercase();
        assert!(
            raw_lower.contains("charset=utf-8") || raw_lower.contains("charset=\"utf-8\""),
            "Email must declare charset=utf-8 in Content-Type. Raw headers:\n{}",
            raw.lines().take(10).collect::<Vec<_>>().join("\n"),
        );
        // Verify the emoji bytes are present in the body
        assert!(
            raw.contains("🎉") || raw.contains("=F0=9F=8E=89"),
            "Email body must contain the emoji (raw or quoted-printable encoded)",
        );
    }

    #[test]
    fn test_mime_type_from_extension() {
        assert_eq!(mime_type_from_extension("report.pdf"), "application/pdf");
        assert_eq!(mime_type_from_extension("page.html"), "text/html");
        assert_eq!(mime_type_from_extension("photo.jpg"), "image/jpeg");
        assert_eq!(mime_type_from_extension("data.csv"), "text/csv");
        assert_eq!(mime_type_from_extension("archive.zip"), "application/zip");
        assert_eq!(
            mime_type_from_extension("mystery"),
            "application/octet-stream"
        );
        assert_eq!(
            mime_type_from_extension("pdf"),
            "application/octet-stream",
            "extensionless file named 'pdf' should not match"
        );
        assert_eq!(
            mime_type_from_extension("doc.XLSX"),
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        );
    }

    #[test]
    fn test_email_with_attachment_is_multipart() {
        let text_part = lettre::message::SinglePart::builder()
            .header(
                lettre::message::header::ContentType::parse("text/plain; charset=utf-8").unwrap(),
            )
            .body("Hello".to_string());

        let attachment = lettre::message::Attachment::new("report.pdf".to_string()).body(
            b"fake pdf content".to_vec(),
            lettre::message::header::ContentType::parse("application/pdf").unwrap(),
        );

        let multipart = lettre::message::MultiPart::mixed()
            .singlepart(text_part)
            .singlepart(attachment);

        let message = lettre::Message::builder()
            .from("sender@example.com".parse().unwrap())
            .to("recipient@example.com".parse().unwrap())
            .subject("With attachment")
            .multipart(multipart)
            .unwrap();

        let formatted = message.formatted();
        let raw = String::from_utf8_lossy(&formatted);
        assert!(
            raw.contains("multipart/mixed"),
            "Email with attachment must be multipart/mixed"
        );
        assert!(
            raw.contains("report.pdf"),
            "Attachment filename must appear in MIME headers"
        );
    }

    #[test]
    fn test_format_byte_size() {
        assert_eq!(format_byte_size(0), "0 bytes");
        assert_eq!(format_byte_size(500), "500 bytes");
        assert_eq!(format_byte_size(1023), "1023 bytes");
        assert_eq!(format_byte_size(1024), "1.0 KB");
        assert_eq!(format_byte_size(1536), "1.5 KB");
        assert_eq!(format_byte_size(1_048_576), "1.0 MB");
        assert_eq!(format_byte_size(2_621_440), "2.5 MB");
    }

    #[test]
    fn test_extract_attachment_info_from_multipart_email() {
        // Build a MIME email with two attachments using lettre
        let text_part = lettre::message::SinglePart::builder()
            .header(
                lettre::message::header::ContentType::parse("text/plain; charset=utf-8").unwrap(),
            )
            .body("Hello, please see attached.".to_string());

        let pdf_attachment = lettre::message::Attachment::new("booking.pdf".to_string()).body(
            b"fake pdf content here".to_vec(),
            lettre::message::header::ContentType::parse("application/pdf").unwrap(),
        );

        let img_attachment = lettre::message::Attachment::new("photo.jpg".to_string()).body(
            b"fake jpeg data".to_vec(),
            lettre::message::header::ContentType::parse("image/jpeg").unwrap(),
        );

        let multipart = lettre::message::MultiPart::mixed()
            .singlepart(text_part)
            .singlepart(pdf_attachment)
            .singlepart(img_attachment);

        let message = lettre::Message::builder()
            .from("sender@example.com".parse().unwrap())
            .to("recipient@example.com".parse().unwrap())
            .subject("With attachments")
            .multipart(multipart)
            .unwrap();

        let raw_bytes = message.formatted();

        // Parse with mail_parser (same as read_email does)
        let parser = mail_parser::MessageParser::default();
        let parsed = parser.parse(&raw_bytes).expect("should parse the email");

        let attachments = extract_attachment_info(&parsed);

        assert_eq!(
            attachments.len(),
            2,
            "should find 2 attachments (text body is not an attachment)"
        );

        // First attachment: PDF
        assert_eq!(attachments[0].filename, "booking.pdf");
        assert_eq!(attachments[0].mime_type, "application/pdf");
        assert!(attachments[0].size > 0);
        assert_eq!(attachments[0].index, 0);

        // Second attachment: JPEG
        assert_eq!(attachments[1].filename, "photo.jpg");
        assert_eq!(attachments[1].mime_type, "image/jpeg");
        assert!(attachments[1].size > 0);
        assert_eq!(attachments[1].index, 1);
    }

    #[test]
    fn test_extract_attachment_info_no_attachments() {
        // Plain text email, no attachments
        let message = lettre::Message::builder()
            .from("sender@example.com".parse().unwrap())
            .to("recipient@example.com".parse().unwrap())
            .subject("Plain text")
            .singlepart(
                lettre::message::SinglePart::builder()
                    .header(
                        lettre::message::header::ContentType::parse("text/plain; charset=utf-8")
                            .unwrap(),
                    )
                    .body("No attachments here.".to_string()),
            )
            .unwrap();

        let raw_bytes = message.formatted();
        let parser = mail_parser::MessageParser::default();
        let parsed = parser.parse(&raw_bytes).expect("should parse");

        let attachments = extract_attachment_info(&parsed);
        assert!(
            attachments.is_empty(),
            "plain text email should have no attachments"
        );
    }

    #[test]
    fn test_email_message_includes_attachments() {
        // Verify the EmailMessage struct carries attachments
        let msg = EmailMessage {
            uid: 42,
            message_id: "<test@example.com>".to_string(),
            from: "sender@example.com".to_string(),
            to: "recipient@example.com".to_string(),
            cc: String::new(),
            subject: "Test".to_string(),
            date: "2026-04-07T12:00:00Z".to_string(),
            body: "Hello".to_string(),
            attachments: vec![EmailAttachmentInfo {
                index: 0,
                filename: "doc.pdf".to_string(),
                mime_type: "application/pdf".to_string(),
                size: 12345,
            }],
        };
        assert_eq!(msg.attachments.len(), 1);
        assert_eq!(msg.attachments[0].filename, "doc.pdf");
    }

    // --- IMAP search query building ---

    #[test]
    fn test_build_imap_query_no_params() {
        assert_eq!(build_imap_query(None, None), "ALL");
    }

    #[test]
    fn test_build_imap_query_search_only() {
        assert_eq!(
            build_imap_query(Some("FROM \"user@example.com\""), None),
            "FROM \"user@example.com\""
        );
    }

    #[test]
    fn test_build_imap_query_since_only() {
        assert_eq!(
            build_imap_query(None, Some("25-Feb-2026")),
            "SINCE 25-Feb-2026"
        );
    }

    #[test]
    fn test_build_imap_query_both() {
        assert_eq!(
            build_imap_query(Some("FROM \"user@example.com\""), Some("25-Feb-2026")),
            "SINCE 25-Feb-2026 FROM \"user@example.com\""
        );
    }

    #[test]
    fn test_build_imap_query_keyword_passthrough() {
        assert_eq!(build_imap_query(Some("UNSEEN"), None), "UNSEEN");
    }

    // --- Non-ASCII sanitization ---

    #[test]
    fn test_sanitize_ascii_only_unchanged() {
        assert_eq!(
            sanitize_search_query("FROM \"user@example.com\""),
            "FROM \"user@example.com\""
        );
    }

    #[test]
    fn test_sanitize_strips_non_ascii_in_quotes() {
        assert_eq!(
            sanitize_search_query("FROM \"Kløfta eSport\""),
            "FROM \"Klfta eSport\""
        );
    }

    #[test]
    fn test_sanitize_preserves_keywords() {
        assert_eq!(sanitize_search_query("UNSEEN"), "UNSEEN");
    }

    #[test]
    fn test_sanitize_multiple_criteria() {
        assert_eq!(
            sanitize_search_query("OR SUBJECT \"Ålborg\" FROM \"café@example.com\""),
            "OR SUBJECT \"lborg\" FROM \"caf@example.com\""
        );
    }

    // --- Client-side search filtering ---

    #[test]
    fn test_filter_from_match() {
        let email = EmailSummary {
            uid: 1,
            message_id: String::new(),
            from: "Kløfta eSport <esport@kloftail.no>".to_string(),
            subject: "Meeting".to_string(),
            date: String::new(),
            preview: String::new(),
        };
        assert!(matches_search_filter(&email, "FROM \"Kløfta eSport\""));
        assert!(matches_search_filter(&email, "FROM \"esport@kloftail.no\""));
        assert!(!matches_search_filter(&email, "FROM \"other@example.com\""));
    }

    #[test]
    fn test_filter_subject_match() {
        let email = EmailSummary {
            uid: 1,
            message_id: String::new(),
            from: "sender@example.com".to_string(),
            subject: "Twitch streaming setup".to_string(),
            date: String::new(),
            preview: String::new(),
        };
        assert!(matches_search_filter(&email, "SUBJECT \"Twitch\""));
        assert!(!matches_search_filter(&email, "SUBJECT \"YouTube\""));
    }

    #[test]
    fn test_filter_case_insensitive() {
        let email = EmailSummary {
            uid: 1,
            message_id: String::new(),
            from: "User@Example.COM".to_string(),
            subject: "IMPORTANT Meeting".to_string(),
            date: String::new(),
            preview: String::new(),
        };
        assert!(matches_search_filter(&email, "FROM \"user@example.com\""));
        assert!(matches_search_filter(&email, "SUBJECT \"important\""));
    }

    #[test]
    fn test_filter_server_only_keywords_pass() {
        let email = EmailSummary {
            uid: 1,
            message_id: String::new(),
            from: "sender@example.com".to_string(),
            subject: "Test".to_string(),
            date: String::new(),
            preview: String::new(),
        };
        assert!(matches_search_filter(&email, "UNSEEN"));
        assert!(matches_search_filter(&email, "ALL"));
    }

    #[test]
    fn test_filter_or_criteria() {
        let email = EmailSummary {
            uid: 1,
            message_id: String::new(),
            from: "esport@kloftail.no".to_string(),
            subject: "Meeting".to_string(),
            date: String::new(),
            preview: String::new(),
        };
        assert!(matches_search_filter(
            &email,
            "OR SUBJECT \"Twitch\" FROM \"esport@kloftail.no\""
        ));
        assert!(!matches_search_filter(
            &email,
            "OR SUBJECT \"Twitch\" FROM \"other@example.com\""
        ));
    }

    // --- Tokenizer ---

    #[test]
    fn test_tokenize_simple() {
        let tokens = tokenize_imap_query("FROM \"user@example.com\"");
        assert_eq!(tokens, vec!["FROM", "\"user@example.com\""]);
    }

    #[test]
    fn test_tokenize_quoted_spaces() {
        let tokens = tokenize_imap_query("FROM \"Kløfta eSport\"");
        assert_eq!(tokens, vec!["FROM", "\"Kløfta eSport\""]);
    }

    #[test]
    fn test_tokenize_or() {
        let tokens = tokenize_imap_query("OR SUBJECT \"test\" FROM \"user@example.com\"");
        assert_eq!(
            tokens,
            vec!["OR", "SUBJECT", "\"test\"", "FROM", "\"user@example.com\""]
        );
    }
}
