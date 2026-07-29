//! EmailClient (SMTP/IMAP) implementation, split out of email.rs.
use super::*;

/// Hard wall-clock bound for one SMTP send (connect + handshake + auth + data).
/// lettre's async transport enforces its own 60s default only on the TCP
/// connect; every later phase is unbounded, so without this outer bound a
/// server that accepts the connection and stalls hangs the send forever.
/// Generous because the budget also covers the DATA upload of multi-MB
/// attachments on slow uplinks — common failures (unreachable route, refused
/// auth) surface much earlier on their own. The frontend's email-send request
/// timeout (`EMAIL_SEND_TIMEOUT_MS` in
/// `crates/lucidos-app/src/api/client/settings.ts`) must stay ABOVE this value
/// PLUS the 30s OAuth-refresh bound that precedes the send (`bounded_http_client`
/// in `core/oauth.rs`) so the engine's specific error reaches the user instead
/// of a generic client-side "request timed out".
const SMTP_SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(80);

/// Hard wall-clock bound for one complete IMAP operation (session setup +
/// select + search/fetch + logout). `IMAP_CONNECT_TIMEOUT` (email.rs) bounds
/// only session setup with a sharper error; async-imap's post-auth command
/// phase is equally unbounded, so a server that stalls AFTER login would
/// otherwise hang the calling chat turn indefinitely. Deliberately generous:
/// the budget also covers a full RFC822 download (attachments run to ~25MB),
/// so it's a stall backstop for the chat turn, not a UX-latency bound — no
/// frontend request timeout depends on it.
const IMAP_OP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Error for an IMAP operation that exceeded `IMAP_OP_TIMEOUT` — names the
/// operation and endpoint per the errors-name-the-entity rule.
fn imap_op_timed_out(account: &EmailAccount, op: &str) -> BoxError {
    format!(
        "IMAP {} via {}:{} did not complete within {}s — the server may be stalled, \
         or the message is too large for the current connection",
        op,
        account.imap_host,
        account.imap_port,
        IMAP_OP_TIMEOUT.as_secs()
    )
    .into()
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
        tokio::time::timeout(
            IMAP_OP_TIMEOUT,
            Self::read_emails_unbounded(account, folder, limit, search, since, oauth_access_token),
        )
        .await
        .map_err(|_| imap_op_timed_out(account, "read"))?
    }

    async fn read_emails_unbounded(
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
        tokio::time::timeout(
            IMAP_OP_TIMEOUT,
            Self::read_email_unbounded(account, uid, folder, oauth_access_token),
        )
        .await
        .map_err(|_| imap_op_timed_out(account, "read"))?
    }

    async fn read_email_unbounded(
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
        tokio::time::timeout(
            IMAP_OP_TIMEOUT,
            Self::fetch_attachment_unbounded(account, uid, attachment_index, folder, oauth_access_token),
        )
        .await
        .map_err(|_| imap_op_timed_out(account, "attachment fetch"))?
    }

    async fn fetch_attachment_unbounded(
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
        Self::send_email_with_timeout(
            account,
            to,
            subject,
            body,
            cc,
            bcc,
            in_reply_to,
            oauth_access_token,
            attachments,
            SMTP_SEND_TIMEOUT,
        )
        .await
    }

    /// `send_email` with an explicit wall-clock bound (tests pass a short one).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn send_email_with_timeout(
        account: &EmailAccount,
        to: &str,
        subject: &str,
        body: &str,
        cc: Option<&str>,
        bcc: Option<&str>,
        in_reply_to: Option<&str>,
        oauth_access_token: Option<&str>,
        attachments: &[EmailAttachment],
        send_timeout: std::time::Duration,
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

        // Outer wall-clock bound — see `SMTP_SEND_TIMEOUT`'s doc for why the
        // transport can't be trusted to time out on its own.
        tokio::time::timeout(send_timeout, transport.send(message))
            .await
            .map_err(|_| {
                format!(
                    "SMTP send via {}:{} timed out after {}s — the server did not respond \
                     (network may block outbound SMTP, or the server is stalled)",
                    account.smtp_host,
                    account.smtp_port,
                    send_timeout.as_secs()
                )
            })??;

        let auth_method = if oauth_access_token.is_some() {
            "XOAUTH2"
        } else {
            "password"
        };
        crate::log!(@Email, "Email sent to {} via {} ({})", to, account.smtp_host, auth_method);

        Ok(message_id)
    }
}
