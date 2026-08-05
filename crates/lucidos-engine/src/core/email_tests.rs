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
        .header(lettre::message::header::ContentType::parse("text/plain; charset=utf-8").unwrap())
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
fn test_extract_attachment_info_from_multipart_email() {
    // Build a MIME email with two attachments using lettre
    let text_part = lettre::message::SinglePart::builder()
        .header(lettre::message::header::ContentType::parse("text/plain; charset=utf-8").unwrap())
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
        sanitize_search_query("FROM \"Café Latté\""),
        "FROM \"Caf Latt\""
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
        from: "Café Latté <team@example.no>".to_string(),
        subject: "Meeting".to_string(),
        date: String::new(),
        preview: String::new(),
    };
    assert!(matches_search_filter(&email, "FROM \"Café Latté\""));
    assert!(matches_search_filter(&email, "FROM \"team@example.no\""));
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
        from: "team@example.no".to_string(),
        subject: "Meeting".to_string(),
        date: String::new(),
        preview: String::new(),
    };
    assert!(matches_search_filter(
        &email,
        "OR SUBJECT \"Twitch\" FROM \"team@example.no\""
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
    let tokens = tokenize_imap_query("FROM \"Café Latté\"");
    assert_eq!(tokens, vec!["FROM", "\"Café Latté\""]);
}

#[test]
fn test_tokenize_or() {
    let tokens = tokenize_imap_query("OR SUBJECT \"test\" FROM \"user@example.com\"");
    assert_eq!(
        tokens,
        vec!["OR", "SUBJECT", "\"test\"", "FROM", "\"user@example.com\""]
    );
}

#[test]
fn debug_redacts_email_account_password() {
    let account = EmailAccount {
        id: uuid::Uuid::nil(),
        name: "work".to_string(),
        email_address: "me@work.com".to_string(),
        imap_host: "imap.work.com".to_string(),
        imap_port: 993,
        smtp_host: "smtp.work.com".to_string(),
        smtp_port: 465,
        username: "me@work.com".to_string(),
        password: "hunter2-super-secret".to_string(),
        use_tls: true,
        require_send_confirmation: false,
        oauth_account_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let dbg = format!("{:?}", account);
    // The password must never appear in a `{:?}` rendering.
    assert!(
        !dbg.contains("hunter2-super-secret"),
        "password leaked: {dbg}"
    );
    assert!(
        dbg.contains("password: \"<redacted>\""),
        "expected redacted password: {dbg}"
    );
    // Non-secret connection fields stay visible for debugging.
    assert!(dbg.contains("imap.work.com"));
    assert!(dbg.contains("me@work.com"));
}

/// Accept TCP connections and never respond — models a stalled mail server
/// (connect succeeds; the protocol greeting never arrives). Held sockets stay
/// open so the client sees silence, not a connection reset.
async fn stalled_listener() -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hold = tokio::spawn(async move {
        let mut open = Vec::new();
        while let Ok((sock, _)) = listener.accept().await {
            open.push(sock); // keep open, stay silent
        }
    });
    (port, hold)
}

/// Minimal plain-TCP account fixture aimed at the given IMAP/SMTP endpoints
/// (plain TCP so a stall test exercises the protocol phase, not TLS).
fn stall_test_account(imap_port: i32, smtp_port: i32) -> EmailAccount {
    EmailAccount {
        id: uuid::Uuid::nil(),
        name: "test".to_string(),
        email_address: "me@example.com".to_string(),
        imap_host: "127.0.0.1".to_string(),
        imap_port,
        smtp_host: "127.0.0.1".to_string(),
        smtp_port,
        username: "me@example.com".to_string(),
        password: "pw".to_string(),
        use_tls: false,
        require_send_confirmation: false,
        oauth_account_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

/// lettre's async SMTP transport applies its 60s timeout only to the TCP
/// connect — the command phase (greeting, EHLO, AUTH, DATA) is unbounded, so a
/// server that accepts the connection and then stalls used to hang the send
/// forever (the frontend gave up at its own 10s with a misleading generic
/// "request timed out" while the engine kept waiting with no log). The outer
/// wall-clock bound in `send_email_with_timeout` is what guarantees the send
/// resolves, with an error that names the SMTP endpoint.
#[tokio::test]
async fn send_email_times_out_with_descriptive_error_when_smtp_stalls() {
    let (port, hold) = stalled_listener().await;
    let account = stall_test_account(993, port as i32);

    let started = std::time::Instant::now();
    let err = EmailClient::send_email_with_timeout(
        &account,
        "to@example.com",
        "subject",
        "body",
        None,
        None,
        None,
        None,
        &[],
        std::time::Duration::from_millis(500),
    )
    .await
    .expect_err("send against a stalled SMTP server must error, not hang");

    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "send must resolve promptly once the bound elapses"
    );
    let msg = err.to_string();
    assert!(msg.contains("timed out"), "got: {msg}");
    // Frontend rule: errors name the entity — the SMTP endpoint here.
    assert!(
        msg.contains(&format!("127.0.0.1:{port}")),
        "error must name the SMTP host:port: {msg}"
    );
    hold.abort();
}

/// Sibling of the SMTP bound: IMAP session setup (connect + greeting + auth)
/// had no timeout on any phase, so email reads against a stalled server hung
/// the calling chat turn indefinitely. (Post-auth stalls are bounded by the
/// whole-operation `IMAP_OP_TIMEOUT` wrapper in `email_client.rs`, the same
/// `tokio::time::timeout` mechanism verified here.)
#[tokio::test]
async fn imap_connect_times_out_with_descriptive_error_when_server_stalls() {
    let (port, hold) = stalled_listener().await;
    let account = stall_test_account(port as i32, 587);

    let started = std::time::Instant::now();
    let err = imap_connect_with_timeout(&account, None, std::time::Duration::from_millis(500))
        .await
        .map(|_| ())
        .expect_err("connect against a silent IMAP server must error, not hang");

    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "connect must resolve promptly once the bound elapses"
    );
    let msg = err.to_string();
    assert!(msg.contains("timed out"), "got: {msg}");
    assert!(
        msg.contains(&format!("127.0.0.1:{port}")),
        "error must name the IMAP host:port: {msg}"
    );
    hold.abort();
}

/// The credential's service name IS the `email_accounts.name` now: `auth_type`
/// carries what the old `email:` prefix said, so nothing spells it twice.
#[test]
fn account_name_is_the_credential_service_name() {
    assert_eq!(EmailStore::account_name_for_credential("work"), "work");
    assert_eq!(
        EmailStore::account_name_for_credential("Telenor"),
        "Telenor"
    );
}

/// The stranded-row case, which real data hits: a workspace holding BOTH an
/// `email:Telenor` mailbox password and a separate `Telenor` credential of
/// another type keeps the prefixed name, because `email_password` may not shadow
/// a name. Its `email_accounts` row is still called `Telenor`, so resolving the
/// account by the service name verbatim would find nothing: the edit form's
/// settings fetch 404s, and the password write silently touches zero rows while
/// reporting success. Temporary measure, removed with the
/// `get_email_password` fallback.
#[test]
fn account_name_strips_a_prefix_the_migration_had_to_leave() {
    assert_eq!(
        EmailStore::account_name_for_credential("email:Telenor"),
        "Telenor"
    );
}

/// Case is preserved: `email_accounts.name` is matched exactly, so folding it
/// would detach the credential from its mailbox.
#[test]
fn account_name_preserves_case() {
    assert_eq!(
        EmailStore::account_name_for_credential("email:MixedCase"),
        "MixedCase"
    );
}
