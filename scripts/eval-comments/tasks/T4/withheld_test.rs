use super::*;
use std::sync::{Arc, Mutex};

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
const FAKE_IMAP_BOUND: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Default)]
struct FakeMailbox {
    seen: bool,
    commands: Vec<String>,
}

// RFC 3501 section 6.4.5: only the BODY.PEEK forms leave the Seen flag alone.
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

// Sets Seen the way a real server does: for a flag-setting fetch on a folder
// opened with SELECT, never on one opened with EXAMINE.
async fn serve_fake_imap(sock: tokio::net::TcpStream, mailbox: Arc<Mutex<FakeMailbox>>) {
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

async fn fake_imap_server() -> (u16, Arc<Mutex<FakeMailbox>>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mailbox = Arc::new(Mutex::new(FakeMailbox::default()));
    let served = mailbox.clone();
    let accept = tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            tokio::spawn(serve_fake_imap(sock, served.clone()));
        }
    });
    (port, mailbox, accept)
}

fn plain_tcp_account(imap_port: i32) -> EmailAccount {
    EmailAccount {
        id: uuid::Uuid::nil(),
        name: "test".to_string(),
        email_address: "me@example.com".to_string(),
        imap_host: "127.0.0.1".to_string(),
        imap_port,
        smtp_host: "127.0.0.1".to_string(),
        smtp_port: 587,
        username: "me@example.com".to_string(),
        password: "pw".to_string(),
        use_tls: false,
        require_send_confirmation: false,
        oauth_account_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

fn assert_left_unread(mailbox: &Mutex<FakeMailbox>) {
    let mailbox = mailbox.lock().unwrap();
    assert!(!mailbox.seen, "the read set \\Seen: {:?}", mailbox.commands);
}

#[tokio::test]
async fn read_email_leaves_the_message_unread() {
    let (port, mailbox, accept) = fake_imap_server().await;
    let account = plain_tcp_account(port as i32);

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
    let account = plain_tcp_account(port as i32);

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
    let account = plain_tcp_account(port as i32);

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
