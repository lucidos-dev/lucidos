//! LLM-facing schemas for email tools (configure/send/read/save attachment).
//! Handlers live in `core::email` + `engine::tools`.
//!
//! Two families, not one, and the split is a capability gate rather than a
//! grouping: see [`configure_email_tools`].

use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

/// Setting an account up, which a workspace with no account still needs.
///
/// Split from [`mailbox_tools`] because this is the only writer of the first
/// `email_accounts` row. The settings UI reaches `EmailStore::upsert` only
/// when EDITING an existing `email_password` credential, and the create path
/// runs a bare `UPDATE` that touches nothing. So gating this on having an
/// account makes email permanently unreachable, which is not what ADR 0088
/// gates: it gates offers the engine would refuse.
pub(super) fn configure_email_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::CONFIGURE_EMAIL.to_string(),
            description: "Configure an email account for sending and reading. web_search the provider's IMAP and SMTP host and port first, and prefer use_oauth, which most providers now require.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Account label (e.g. 'Gmail')."
                    },
                    "email_address": {
                        "type": "string",
                        "description": "Email address."
                    },
                    "imap_host": {
                        "type": "string",
                        "description": "IMAP hostname."
                    },
                    "imap_port": {
                        "type": "integer",
                        "description": "IMAP port (default 993 for TLS)."
                    },
                    "smtp_host": {
                        "type": "string",
                        "description": "SMTP hostname."
                    },
                    "smtp_port": {
                        "type": "integer",
                        "description": "SMTP port (default 587 for STARTTLS)."
                    },
                    "username": {
                        "type": "string",
                        "description": "Login username (defaults to email_address)."
                    },
                    "use_tls": {
                        "type": "boolean",
                        "description": "Use TLS (default true)."
                    },
                    "require_send_confirmation": {
                        "type": "boolean",
                        "description": "Require user confirmation before sending (default true)."
                    },
                    "use_oauth": {
                        "type": "string",
                        "description": "OAuth provider for SMTP auth (XOAUTH2 instead of a password); must already be connected with connect_oauth_account."
                    }
                },
                "required": ["name", "email_address", "imap_host", "smtp_host"]
            }),
        },
    ]
}

/// Everything that operates on an account that already exists. Offered only
/// where the workspace has one (ADR 0088), since every one of these resolves
/// an account first and errors without one.
pub(super) fn mailbox_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::SEND_EMAIL.to_string(),
            description: "Send an email. If the account requires confirmation the user sees a preview and must approve first.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "to": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Recipient addresses."
                    },
                    "subject": {
                        "type": "string",
                        "description": "Subject line."
                    },
                    "body": {
                        "type": "string",
                        "description": "Body, plain text."
                    },
                    "cc": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "CC recipients."
                    },
                    "bcc": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "BCC recipients."
                    },
                    "reply_to_message_id": {
                        "type": "string",
                        "description": "Message-ID to reply to, for threading."
                    },
                    "account": {
                        "type": "string",
                        "description": "Account name (default account if omitted)."
                    },
                    "attachments": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Paths relative to data/ to attach; MIME type comes from the extension."
                    }
                },
                "required": ["to", "subject", "body"]
            }),
        },
        ToolDefinition {
            name: tn::READ_EMAILS.to_string(),
            description: "Fetch recent emails: sender, subject, date and preview. read_email with a UID gets the full body.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "folder": {
                        "type": "string",
                        "description": "IMAP folder (default 'INBOX')."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max emails (default 10, max 50)."
                    },
                    "search": {
                        "type": "string",
                        "description": "IMAP search query (e.g. 'UNSEEN')."
                    },
                    "since": {
                        "type": "string",
                        "description": "Only emails since this date, IMAP format: '25-Feb-2026'."
                    },
                    "account": {
                        "type": "string",
                        "description": "Account name (default account if omitted)."
                    }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: tn::READ_EMAIL.to_string(),
            description: "Read one email in full by its UID, from read_emails results.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "uid": {
                        "type": "integer",
                        "description": "Email UID from read_emails."
                    },
                    "folder": {
                        "type": "string",
                        "description": "IMAP folder (default 'INBOX')."
                    },
                    "account": {
                        "type": "string",
                        "description": "Account name (default account if omitted)."
                    }
                },
                "required": ["uid"]
            }),
        },
        ToolDefinition {
            name: tn::SAVE_EMAIL_ATTACHMENT.to_string(),
            description: "Save an email attachment, landing in artifacts/imported/email/ by default, after read_email has listed them. A PDF is saved as a binary artifact with no text extraction.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "uid": {
                        "type": "integer",
                        "description": "Email UID from read_email."
                    },
                    "attachment_index": {
                        "type": "integer",
                        "description": "Attachment index from read_email, 0-based."
                    },
                    "folder": {
                        "type": "string",
                        "description": "IMAP folder (default 'INBOX')."
                    },
                    "destination": {
                        "type": "string",
                        "description": "Destination relative to data/artifacts/ (default 'imported/email/<filename>')."
                    },
                    "account": {
                        "type": "string",
                        "description": "Account name (default account if omitted)."
                    }
                },
                "required": ["uid", "attachment_index"]
            }),
        },
    ]
}
