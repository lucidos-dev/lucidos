use super::*;

#[test]
fn test_xoauth2_authenticator_format() {
    use async_imap::Authenticator;

    let auth = super::XOAuth2 {
        user: "user@outlook.com".to_string(),
        access_token: "ya29.test-token-123".to_string(),
    };
    let response = (&auth).process(b"");

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
    let raw_lower = raw.to_lowercase();
    assert!(
        raw_lower.contains("charset=utf-8") || raw_lower.contains("charset=\"utf-8\""),
        "Email must declare charset=utf-8 in Content-Type. Raw headers:\n{}",
        raw.lines().take(10).collect::<Vec<_>>().join("\n"),
    );
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

    let parser = mail_parser::MessageParser::default();
    let parsed = parser.parse(&raw_bytes).expect("should parse the email");

    let attachments = extract_attachment_info(&parsed);

    assert_eq!(
        attachments.len(),
        2,
        "should find 2 attachments (text body is not an attachment)"
    );

    assert_eq!(attachments[0].filename, "booking.pdf");
    assert_eq!(attachments[0].mime_type, "application/pdf");
    assert!(attachments[0].size > 0);
    assert_eq!(attachments[0].index, 0);

    assert_eq!(attachments[1].filename, "photo.jpg");
    assert_eq!(attachments[1].mime_type, "image/jpeg");
    assert!(attachments[1].size > 0);
    assert_eq!(attachments[1].index, 1);
}

#[test]
fn test_extract_attachment_info_no_attachments() {
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

#[test]
fn imap_line_break_is_refused_in_a_search_fragment() {
    let injection = "ALL\r\nA1 STORE 1:* +FLAGS (\\Deleted)";
    let err = reject_imap_line_break("search", injection)
        .expect_err("a CRLF-carrying search must be refused, not sent");
    let msg = err.to_string();
    assert!(
        msg.contains("search"),
        "the error names the argument: {msg}"
    );
    assert!(msg.contains("line break"), "got: {msg}");

    assert!(reject_imap_line_break("search", "ALL\nA1 EXPUNGE").is_err());
    assert!(reject_imap_line_break("since", "25-Feb-2026\r\nA1 EXPUNGE").is_err());
}

#[test]
fn imap_line_break_guard_accepts_every_real_criterion() {
    for ok in [
        "ALL",
        "UNSEEN",
        "FROM \"user@example.com\"",
        "OR SUBJECT \"Ålborg\" FROM \"café@example.com\"",
        "25-Feb-2026",
        "",
    ] {
        assert!(
            reject_imap_line_break("search", ok).is_ok(),
            "legitimate criterion must pass: {ok:?}"
        );
    }
}

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
        password: MailPassword::Saved("hunter2-super-secret".to_string()),
        use_tls: true,
        require_send_confirmation: false,
        oauth_account_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let dbg = format!("{:?}", account);
    assert!(
        !dbg.contains("hunter2-super-secret"),
        "password leaked: {dbg}"
    );
    assert!(
        dbg.contains("password: \"<redacted>\""),
        "expected redacted password: {dbg}"
    );
    assert!(dbg.contains("imap.work.com"));
    assert!(dbg.contains("me@work.com"));
}

async fn stalled_listener() -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hold = tokio::spawn(async move {
        let mut open = Vec::new();
        while let Ok((sock, _)) = listener.accept().await {
            open.push(sock);
        }
    });
    (port, hold)
}

fn plain_tcp_test_account(imap_port: i32, smtp_port: i32) -> EmailAccount {
    EmailAccount {
        id: uuid::Uuid::nil(),
        name: "test".to_string(),
        email_address: "me@example.com".to_string(),
        imap_host: "127.0.0.1".to_string(),
        imap_port,
        smtp_host: "127.0.0.1".to_string(),
        smtp_port,
        username: "me@example.com".to_string(),
        password: MailPassword::Saved("pw".to_string()),
        use_tls: false,
        require_send_confirmation: false,
        oauth_account_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn send_email_times_out_with_descriptive_error_when_smtp_stalls() {
    let (port, hold) = stalled_listener().await;
    let account = plain_tcp_test_account(993, port as i32);

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
    assert!(
        msg.contains(&format!("127.0.0.1:{port}")),
        "error must name the SMTP host:port: {msg}"
    );
    hold.abort();
}

#[tokio::test]
async fn imap_connect_times_out_with_descriptive_error_when_server_stalls() {
    let (port, hold) = stalled_listener().await;
    let account = plain_tcp_test_account(port as i32, 587);

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

const FAKE_MESSAGE: &[u8] = b"From: sender@example.com\r\n\
To: me@example.com\r\n\
Subject: Quarterly report\r\n\
Message-ID: <msg-42@example.com>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\
\r\n\
--b\r\n\
Content-Type: text/plain\r\n\
\r\n\
See attached.\r\n\
--b\r\n\
Content-Type: text/plain; name=\"notes.txt\"\r\n\
Content-Disposition: attachment; filename=\"notes.txt\"\r\n\
\r\n\
attachment body\r\n\
--b--\r\n";

const FAKE_UID: u32 = 42;

#[derive(Default)]
struct FakeMailbox {
    seen: bool,
    commands: Vec<String>,
}

fn fetch_sets_seen(command: &str) -> bool {
    let upper = command.to_ascii_uppercase();
    if !upper.starts_with("UID FETCH") {
        return false;
    }
    let without_peek = upper.replace("BODY.PEEK[", "");
    without_peek.contains("BODY[")
        || without_peek
            .split([' ', '(', ')'])
            .any(|item| item == "RFC822" || item == "RFC822.TEXT")
}

async fn serve_fake_imap(
    sock: tokio::net::TcpStream,
    mailbox: std::sync::Arc<std::sync::Mutex<FakeMailbox>>,
) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let (read, mut write) = sock.into_split();
    let mut lines = BufReader::new(read).lines();
    write.write_all(b"* OK fake IMAP ready\r\n").await.unwrap();
    let mut read_only = true;

    while let Ok(Some(line)) = lines.next_line().await {
        let (tag, command) = line.split_once(' ').unwrap_or((line.as_str(), ""));
        let upper = command.to_ascii_uppercase();
        mailbox.lock().unwrap().commands.push(command.to_string());

        let mut reply = Vec::new();
        if upper.starts_with("SELECT") || upper.starts_with("EXAMINE") {
            read_only = upper.starts_with("EXAMINE");
            reply.extend_from_slice(b"* FLAGS (\\Seen)\r\n* 1 EXISTS\r\n");
        } else if upper.starts_with("UID SEARCH") {
            reply.extend_from_slice(format!("* SEARCH {FAKE_UID}\r\n").as_bytes());
        } else if upper.starts_with("UID FETCH") {
            if fetch_sets_seen(command) && !read_only {
                mailbox.lock().unwrap().seen = true;
            }
            let item = if upper.contains("BODY.PEEK[]") || upper.contains("BODY[]") {
                Some("BODY[]")
            } else if upper.contains("RFC822") {
                Some("RFC822")
            } else {
                None
            };
            match item {
                Some(item) => {
                    let head = format!(
                        "* 1 FETCH (UID {FAKE_UID} {item} {{{}}}\r\n",
                        FAKE_MESSAGE.len()
                    );
                    reply.extend_from_slice(head.as_bytes());
                    reply.extend_from_slice(FAKE_MESSAGE);
                    reply.extend_from_slice(b")\r\n");
                }
                None => {
                    reply.extend_from_slice(format!("* 1 FETCH (UID {FAKE_UID})\r\n").as_bytes())
                }
            }
        } else if upper.starts_with("LOGOUT") {
            reply.extend_from_slice(b"* BYE\r\n");
        }
        reply.extend_from_slice(format!("{tag} OK done\r\n").as_bytes());
        write.write_all(&reply).await.unwrap();
        if upper.starts_with("LOGOUT") {
            return;
        }
    }
}

async fn fake_imap_server() -> (
    u16,
    std::sync::Arc<std::sync::Mutex<FakeMailbox>>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mailbox = std::sync::Arc::new(std::sync::Mutex::new(FakeMailbox::default()));
    let served = mailbox.clone();
    let accept = tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            tokio::spawn(serve_fake_imap(sock, served.clone()));
        }
    });
    (port, mailbox, accept)
}

fn assert_left_unread(mailbox: &std::sync::Mutex<FakeMailbox>) {
    let mailbox = mailbox.lock().unwrap();
    assert!(!mailbox.seen, "the read set \\Seen: {:?}", mailbox.commands);
    assert!(
        !mailbox.commands.iter().any(|c| fetch_sets_seen(c)),
        "a fetch would set \\Seen on a SELECTed folder: {:?}",
        mailbox.commands
    );
    assert!(
        mailbox.commands.iter().any(|c| c.starts_with("EXAMINE")),
        "the folder must open read-only: {:?}",
        mailbox.commands
    );
}

const FAKE_IMAP_BOUND: std::time::Duration = std::time::Duration::from_secs(10);

#[tokio::test]
async fn read_email_leaves_the_message_unread() {
    let (port, mailbox, accept) = fake_imap_server().await;
    let account = plain_tcp_test_account(port as i32, 587);

    let email = tokio::time::timeout(
        FAKE_IMAP_BOUND,
        EmailClient::read_email(&account, FAKE_UID, None, None),
    )
    .await
    .expect("read against the fake server must finish")
    .expect("read must succeed");

    assert_eq!(email.subject, "Quarterly report");
    assert!(email.body.contains("See attached."), "body: {}", email.body);
    assert_eq!(email.attachments.len(), 1);
    assert_left_unread(&mailbox);
    accept.abort();
}

#[tokio::test]
async fn fetch_attachment_leaves_the_message_unread() {
    let (port, mailbox, accept) = fake_imap_server().await;
    let account = plain_tcp_test_account(port as i32, 587);

    let (filename, _mime, data) = tokio::time::timeout(
        FAKE_IMAP_BOUND,
        EmailClient::fetch_attachment(&account, FAKE_UID, 0, None, None),
    )
    .await
    .expect("attachment fetch against the fake server must finish")
    .expect("attachment fetch must succeed");

    assert_eq!(filename, "notes.txt");
    assert_eq!(data, b"attachment body");
    assert_left_unread(&mailbox);
    accept.abort();
}

#[tokio::test]
async fn read_emails_leaves_the_messages_unread() {
    let (port, mailbox, accept) = fake_imap_server().await;
    let account = plain_tcp_test_account(port as i32, 587);

    tokio::time::timeout(
        FAKE_IMAP_BOUND,
        EmailClient::read_emails(&account, None, None, None, None, None),
    )
    .await
    .expect("listing against the fake server must finish")
    .expect("listing must succeed");

    assert_left_unread(&mailbox);
    accept.abort();
}

#[test]
fn account_name_is_the_credential_service_name() {
    assert_eq!(EmailStore::account_name_for_credential("work"), "work");
    assert_eq!(
        EmailStore::account_name_for_credential("ExampleMail"),
        "ExampleMail"
    );
}

#[test]
fn account_name_strips_a_prefix_the_migration_had_to_leave() {
    assert_eq!(
        EmailStore::account_name_for_credential("email:ExampleMail"),
        "ExampleMail"
    );
}

#[test]
fn account_name_preserves_case() {
    assert_eq!(
        EmailStore::account_name_for_credential("email:MixedCase"),
        "MixedCase"
    );
}

#[test]
fn an_attachment_cannot_name_the_gitignored_data_root() {
    for path in [".env", "data/.env", "postgres/pg_hba.conf", "secrets.txt"] {
        let err = EmailAttachment::validate_paths(&[path.to_string()])
            .expect_err("data-root attachment must be refused");
        assert!(
            err.contains("typed subdirectories"),
            "wrong refusal for {path}: {err}"
        );
    }
}

#[test]
fn an_attachment_under_a_typed_subdirectory_is_accepted() {
    let validated = EmailAttachment::validate_paths(&[
        "artifacts/report.pdf".to_string(),
        "data/knowhow/notes.md".to_string(),
    ])
    .expect("typed attachments are allowed");

    assert_eq!(validated[0].rel_path, "artifacts/report.pdf");
    assert_eq!(validated[0].filename, "report.pdf");
    assert_eq!(validated[1].rel_path, "knowhow/notes.md");
    assert_eq!(validated[1].filename, "notes.md");
}

#[test]
fn an_attachment_cannot_traverse_out_of_a_typed_subdirectory() {
    let err = EmailAttachment::validate_paths(&["artifacts/../.env".to_string()])
        .expect_err("traversal must be refused");
    assert!(err.contains("no '..' components"), "wrong refusal: {err}");
}

#[test]
fn an_attachment_that_links_out_of_its_tree_is_refused() {
    use std::os::unix::fs::symlink;
    let ws = tempfile::tempdir().unwrap();
    let artifacts = ws.path().join("data/artifacts");
    std::fs::create_dir_all(&artifacts).unwrap();
    std::fs::write(artifacts.join("report.pdf"), "mine").unwrap();
    std::fs::write(ws.path().join("data/.env"), "KEY=1").unwrap();
    std::fs::write(ws.path().join("secret.txt"), "private").unwrap();
    symlink(ws.path().join("data/.env"), artifacts.join("env.txt")).unwrap();
    symlink(ws.path().join("secret.txt"), artifacts.join("out.txt")).unwrap();
    symlink(artifacts.join("report.pdf"), artifacts.join("alias.pdf")).unwrap();

    for path in ["artifacts/env.txt", "artifacts/out.txt"] {
        let Err(err) = EmailAttachment::read_from_workspace(ws.path(), &[path.to_string()]) else {
            panic!("{path} links out of its tree and was attached");
        };
        assert!(err.contains("symlink"), "wrong refusal for {path}: {err}");
    }
    let read = EmailAttachment::read_from_workspace(
        ws.path(),
        &[
            "artifacts/report.pdf".to_string(),
            "artifacts/alias.pdf".to_string(),
            "artifacts//report.pdf".to_string(),
        ],
    )
    .expect("a file inside its tree still attaches");
    for attachment in &read {
        assert_eq!(attachment.data, b"mine");
    }
}
