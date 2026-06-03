//! LLM-facing schemas for email tools (configure/send/read/save attachment).
//! Handlers live in `core::email` + `engine::tools`.


use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

pub(super) fn email_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::CONFIGURE_EMAIL.to_string(),
            description: "Configure an email account for sending and reading email. Use web_search to look up the provider's IMAP/SMTP host and port before calling this. For authentication, prefer use_oauth if an OAuth account is connected (many providers require it now), otherwise fall back to app password.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Account label (e.g., 'Gmail', 'Work', 'Personal')"
                    },
                    "email_address": {
                        "type": "string",
                        "description": "Email address (e.g., 'user@gmail.com')"
                    },
                    "imap_host": {
                        "type": "string",
                        "description": "IMAP server hostname"
                    },
                    "imap_port": {
                        "type": "integer",
                        "description": "IMAP server port (default: 993 for TLS)"
                    },
                    "smtp_host": {
                        "type": "string",
                        "description": "SMTP server hostname"
                    },
                    "smtp_port": {
                        "type": "integer",
                        "description": "SMTP server port (default: 587 for STARTTLS)"
                    },
                    "username": {
                        "type": "string",
                        "description": "Login username (defaults to email_address if omitted)"
                    },
                    "use_tls": {
                        "type": "boolean",
                        "description": "Use TLS for connections (default: true)"
                    },
                    "require_send_confirmation": {
                        "type": "boolean",
                        "description": "Require user confirmation before sending (default: true)"
                    },
                    "use_oauth": {
                        "type": "string",
                        "description": "OAuth provider name to use for SMTP authentication (e.g., 'microsoft', 'google'). Uses XOAUTH2 instead of password auth. The provider must already be connected via connect_oauth_account."
                    }
                },
                "required": ["name", "email_address", "imap_host", "smtp_host"]
            }),
        },
        ToolDefinition {
            name: tn::SEND_EMAIL.to_string(),
            description: "Send an email. If the account requires confirmation, the user will see a preview and must approve before sending.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "to": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Recipient email addresses"
                    },
                    "subject": {
                        "type": "string",
                        "description": "Email subject line"
                    },
                    "body": {
                        "type": "string",
                        "description": "Email body (plain text)"
                    },
                    "cc": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "CC recipients"
                    },
                    "bcc": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "BCC recipients"
                    },
                    "reply_to_message_id": {
                        "type": "string",
                        "description": "Message-ID to reply to (for threading)"
                    },
                    "account": {
                        "type": "string",
                        "description": "Account name (uses default if omitted)"
                    },
                    "attachments": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "File paths relative to data/ to attach (e.g. 'artifacts/projects/report.pdf'). MIME type is auto-detected from extension."
                    }
                },
                "required": ["to", "subject", "body"]
            }),
        },
        ToolDefinition {
            name: tn::READ_EMAILS.to_string(),
            description: "Fetch recent emails from the inbox. Returns a list with sender, subject, date, and preview. Use read_email with a specific UID to get the full message body.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "folder": {
                        "type": "string",
                        "description": "IMAP folder (default: 'INBOX')"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max emails to return (default: 10, max: 50)"
                    },
                    "search": {
                        "type": "string",
                        "description": "IMAP search query (e.g., 'FROM user@example.com', 'SUBJECT meeting', 'UNSEEN')"
                    },
                    "since": {
                        "type": "string",
                        "description": "Only emails since this date (format: '25-Feb-2026' — IMAP date format)"
                    },
                    "account": {
                        "type": "string",
                        "description": "Account name (uses default if omitted)"
                    }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: tn::READ_EMAIL.to_string(),
            description: "Read the full content of a single email by its UID (from read_emails results).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "uid": {
                        "type": "integer",
                        "description": "Email UID from read_emails results"
                    },
                    "folder": {
                        "type": "string",
                        "description": "IMAP folder (default: 'INBOX')"
                    },
                    "account": {
                        "type": "string",
                        "description": "Account name (uses default if omitted)"
                    }
                },
                "required": ["uid"]
            }),
        },
        ToolDefinition {
            name: tn::SAVE_EMAIL_ATTACHMENT.to_string(),
            description: "Save an email attachment to the workspace. Use after read_email shows attachments. PDFs are saved as binary artifacts; text extraction is not supported. Files are saved to data/artifacts/imported/email/ by default.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "uid": {
                        "type": "integer",
                        "description": "Email UID (from read_email results)"
                    },
                    "attachment_index": {
                        "type": "integer",
                        "description": "Attachment index (from read_email attachments list, 0-based)"
                    },
                    "folder": {
                        "type": "string",
                        "description": "IMAP folder (default: 'INBOX')"
                    },
                    "destination": {
                        "type": "string",
                        "description": "Destination path relative to data/artifacts/ (default: 'imported/email/<filename>')"
                    },
                    "account": {
                        "type": "string",
                        "description": "Account name (uses default if omitted)"
                    }
                },
                "required": ["uid", "attachment_index"]
            }),
        },
    ]
}
