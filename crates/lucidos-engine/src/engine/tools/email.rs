use super::super::LucidosEngine;
use super::credentials::credential_request_payload;
use crate::api::is_path_traversal;
use crate::core::format_byte_size;
use crate::core::oauth;
use crate::core::OAuthStore;
use crate::core::WriteAnnouncement;
use crate::llm::tool_names as tn;
use uuid::Uuid;

impl LucidosEngine {
    /// Resolve the email account to use for a tool call.
    /// If `account_name` is Some, look it up by name; otherwise use the default.
    async fn resolve_email_account(
        &self,
        account_name: Option<&str>,
    ) -> Result<crate::core::EmailAccount, String> {
        use crate::core::EmailStore;

        let account = if let Some(name) = account_name {
            EmailStore::get(&self.pool, name)
                .await
                .map_err(|e| format!("Error: Failed to look up email account: {}", e))?
                .ok_or_else(|| {
                    format!(
                        "Error: No email account named '{}'. Use configure_email to set one up.",
                        name
                    )
                })?
        } else {
            EmailStore::get_default(&self.pool)
                .await
                .map_err(|e| format!("Error: Failed to look up email accounts: {}", e))?
                .ok_or_else(|| {
                    "Error: No email accounts configured. Use configure_email to set one up."
                        .to_string()
                })?
        };

        // Only require password if no OAuth account is linked
        if account.oauth_account_id.is_none() && account.password.is_empty() {
            return Err(format!("Error: Email account '{}' has no password configured. Either enter an app password or connect an OAuth account.", account.name));
        }

        Ok(account)
    }

    /// Resolve an OAuth access token for an email account, refreshing if needed.
    /// Returns None if no OAuth account is linked.
    async fn resolve_email_oauth_token(
        &self,
        account: &crate::core::EmailAccount,
    ) -> Option<String> {
        let oauth_id = account.oauth_account_id?;

        let mut oauth_account = match OAuthStore::get_by_id(&self.pool, oauth_id).await {
            Ok(Some(a)) => a,
            Ok(None) => {
                log!("[Email] Linked OAuth account {} not found", oauth_id);
                return None;
            }
            Err(e) => {
                log!("[Email] Failed to fetch OAuth account {}: {}", oauth_id, e);
                return None;
            }
        };

        // Refresh if expired or expiring within 60s
        match oauth::refresh_oauth_if_needed(&self.pool, &mut oauth_account).await {
            Ok(()) => {}
            Err(e) => log!(
                "[Email] OAuth token refresh failed for {}: {}",
                oauth_account.provider,
                e
            ),
        }

        Some(oauth_account.access_token)
    }

    /// The base provider whose token endpoint issues `provider`'s tokens, from
    /// the `token_url` its stored client credential names. `None` when the
    /// credential is missing, unreadable or points at an unknown issuer.
    async fn oauth_token_issuer(&self, provider: &str) -> Option<&'static str> {
        let service = oauth::client_provider_name(provider);
        let credential =
            match crate::core::CredentialStore::get_oauth_client(&self.pool, &service).await {
                Ok(found) => found?,
                Err(e) => {
                    log!("[Email] Failed to read the {} OAuth client: {}", service, e);
                    return None;
                }
            };
        let config: serde_json::Value = serde_json::from_str(&credential.auth_value).ok()?;
        oauth::provider_for_url(config["token_url"].as_str()?)
    }

    pub(crate) async fn execute_email_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
        _request_id: Uuid,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match name {
            tn::CONFIGURE_EMAIL => {
                use crate::core::EmailStore;

                let name = args["name"].as_str().unwrap_or("");
                let email_address = args["email_address"].as_str().unwrap_or("");
                let imap_host = args["imap_host"].as_str().unwrap_or("");
                let smtp_host = args["smtp_host"].as_str().unwrap_or("");
                let use_tls = args["use_tls"].as_bool().unwrap_or(true);
                let require_confirm = args["require_send_confirmation"].as_bool().unwrap_or(true);
                let use_oauth = args.get("use_oauth").and_then(|v| v.as_str());

                if name.is_empty()
                    || email_address.is_empty()
                    || imap_host.is_empty()
                    || smtp_host.is_empty()
                {
                    return Ok(
                        "Error: name, email_address, imap_host, and smtp_host are required"
                            .to_string(),
                    );
                }

                // Read first: a lookup that FAILED is not "no such account". Read
                // as one, a DB blip re-prompts the user for a password already
                // on file, or skips the redirect check below.
                let existing = match EmailStore::get(&self.pool, name).await {
                    Ok(found) => found,
                    Err(e) => {
                        return Ok(format!(
                            "Error: could not read the '{}' email account: {}. Not changing it until this read works.",
                            name, e
                        ))
                    }
                };
                // An argument the model left out keeps the stored value, so an
                // update to one field never rewrites the login or a port.
                let imap_port = args["imap_port"]
                    .as_i64()
                    .map(|p| p as i32)
                    .or(existing.as_ref().map(|a| a.imap_port))
                    .unwrap_or(993);
                let smtp_port = args["smtp_port"]
                    .as_i64()
                    .map(|p| p as i32)
                    .or(existing.as_ref().map(|a| a.smtp_port))
                    .unwrap_or(587);
                let username = args["username"]
                    .as_str()
                    .or(existing.as_ref().map(|a| a.username.as_str()))
                    .unwrap_or(email_address)
                    .to_string();
                let username = username.as_str();
                if let Some(refusal) = existing.as_ref().and_then(|account| {
                    secret_redirect_refusal(
                        account,
                        MailDestination {
                            imap_host,
                            imap_port,
                            smtp_host,
                            smtp_port,
                            username,
                            use_tls,
                        },
                    )
                }) {
                    return Ok(refusal);
                }

                // If use_oauth is set, find the matching OAuth account and link it.
                if let Some(oauth_provider) = use_oauth {
                    // Resolved BEFORE the upsert. The other way round, an
                    // unconnected provider still rewrote the account's address,
                    // hosts and ports, then reported the error.
                    let oauth_account = match OAuthStore::get_by_provider(
                        &self.pool,
                        oauth_provider,
                    )
                    .await
                    {
                        Ok(Some(account)) => account,
                        Ok(None) => {
                            return Ok(format!(
                                "Error: No OAuth account connected for provider '{}'. Use connect_oauth_account first.",
                                oauth_provider
                            ));
                        }
                        Err(e) => {
                            return Ok(format!("Error: Failed to look up OAuth account: {}", e))
                        }
                    };
                    let already_linked = existing
                        .as_ref()
                        .is_some_and(|a| a.oauth_account_id == Some(oauth_account.id));
                    if !already_linked {
                        let issuer = self.oauth_token_issuer(oauth_provider).await;
                        if let Some(refusal) = oauth_link_refusal(
                            oauth_provider,
                            issuer,
                            &MailDestination {
                                imap_host,
                                imap_port,
                                smtp_host,
                                smtp_port,
                                username,
                                use_tls,
                            },
                        ) {
                            return Ok(refusal);
                        }
                    }

                    EmailStore::upsert(
                        &self.pool,
                        name,
                        email_address,
                        imap_host,
                        imap_port,
                        smtp_host,
                        smtp_port,
                        username,
                        use_tls,
                        require_confirm,
                    )
                    .await
                    .map_err(|e| format!("Error: {}", e))?;

                    EmailStore::link_oauth(&self.pool, name, Some(oauth_account.id))
                        .await
                        .map_err(|e| format!("Error: {}", e))?;
                    return Ok(format!(
                        "Email account '{}' configured with OAuth ({}) for SMTP authentication. No app password needed. Ready to send/receive.",
                        name, oauth_provider
                    ));
                }

                // An account that already holds a password or OAuth link keeps it.
                if let Some(existing) = existing {
                    if !existing.password.is_empty() || existing.oauth_account_id.is_some() {
                        EmailStore::upsert(
                            &self.pool,
                            name,
                            email_address,
                            imap_host,
                            imap_port,
                            smtp_host,
                            smtp_port,
                            username,
                            use_tls,
                            require_confirm,
                        )
                        .await
                        .map_err(|e| format!("Error: {}", e))?;
                        let auth_note = if existing.oauth_account_id.is_some() {
                            "OAuth authentication unchanged."
                        } else {
                            "Password unchanged."
                        };
                        return Ok(format!(
                            "Email account '{}' updated. {} Ready to send/receive.",
                            name, auth_note
                        ));
                    }
                }

                EmailStore::upsert(
                    &self.pool,
                    name,
                    email_address,
                    imap_host,
                    imap_port,
                    smtp_host,
                    smtp_port,
                    username,
                    use_tls,
                    require_confirm,
                )
                .await
                .map_err(|e| format!("Error: {}", e))?;

                Ok(credential_request_payload(
                    // The account name verbatim: `auth_type = email_password`
                    // is what marks this as a mailbox password, so the name no
                    // longer carries an `email:` prefix to say the same thing
                    // (`20260805134838_drop_credential_name_prefixes_use_auth_type.sql`).
                    // It must match `email_accounts.name` byte for byte, which
                    // is how `update_password` finds the row.
                    name,
                    &format!("Enter the app password for {}", email_address),
                    &[format!("smtp://{}", smtp_host)],
                    "email_password",
                ))
            }
            tn::SEND_EMAIL => {
                use crate::core::email::EmailClient;

                let to: Vec<String> = args["to"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let subject = args["subject"].as_str().unwrap_or("");
                let body = args["body"].as_str().unwrap_or("");
                let cc: Vec<String> = args
                    .get("cc")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let bcc: Vec<String> = args
                    .get("bcc")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let reply_to = args.get("reply_to_message_id").and_then(|v| v.as_str());
                let account_name = args.get("account").and_then(|v| v.as_str());
                let attachment_paths: Vec<String> = args
                    .get("attachments")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                if to.is_empty() || subject.is_empty() {
                    return Ok("Error: to and subject are required".to_string());
                }

                // Validate attachment paths early (before account resolution / confirmation)
                let validated =
                    crate::core::email::EmailAttachment::validate_paths(&attachment_paths)
                        .map_err(|e| format!("Error: {}", e))?;

                let account = match self.resolve_email_account(account_name).await {
                    Ok(a) => a,
                    Err(e) => return Ok(e),
                };

                // Trigger runs are unattended, so skip send confirmation.
                let is_trigger = crate::scheduler::user_tasks::current_trigger_id().is_some();
                if account.require_send_confirmation && !is_trigger {
                    let attachment_names: Vec<String> =
                        validated.iter().map(|v| v.filename.clone()).collect();
                    let draft = serde_json::json!({
                        "to": to,
                        "subject": subject,
                        "body": body,
                        "cc": cc,
                        "bcc": bcc,
                        "reply_to_message_id": reply_to,
                        "account": account.name,
                        "from": account.email_address,
                        "attachments": attachment_paths,
                        "attachment_names": attachment_names,
                    });
                    return Ok(format!("[EMAIL_CONFIRM]{}", draft));
                }

                // Read file data only when actually sending (not for confirmation preview)
                let attachments = crate::core::email::EmailAttachment::read_from_workspace(
                    &self.workspace_path,
                    &attachment_paths,
                )
                .map_err(|e| format!("Error: {}", e))?;

                let oauth_token = self.resolve_email_oauth_token(&account).await;

                let to_str = to.join(", ");
                let cc_str = cc.join(", ");
                let bcc_str = bcc.join(", ");
                let cc_opt = if cc_str.is_empty() {
                    None
                } else {
                    Some(cc_str.as_str())
                };
                let bcc_opt = if bcc_str.is_empty() {
                    None
                } else {
                    Some(bcc_str.as_str())
                };

                match EmailClient::send_email(
                    &account,
                    &to_str,
                    subject,
                    body,
                    cc_opt,
                    bcc_opt,
                    reply_to,
                    oauth_token.as_deref(),
                    &attachments,
                )
                .await
                {
                    Ok(result) => Ok(result),
                    Err(e) => Ok(format!("Error: Failed to send email: {}", e)),
                }
            }
            tn::READ_EMAILS => {
                use crate::core::email::EmailClient;

                let folder = args.get("folder").and_then(|v| v.as_str());
                let limit = args
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .map(|l| l.min(50) as u32);
                let search = args.get("search").and_then(|v| v.as_str());
                let since = args.get("since").and_then(|v| v.as_str());
                let account_name = args.get("account").and_then(|v| v.as_str());

                let account = match self.resolve_email_account(account_name).await {
                    Ok(a) => a,
                    Err(e) => return Ok(e),
                };

                let oauth_token = self.resolve_email_oauth_token(&account).await;

                match EmailClient::read_emails(
                    &account,
                    folder,
                    limit,
                    search,
                    since,
                    oauth_token.as_deref(),
                )
                .await
                {
                    Ok(emails) => {
                        if emails.is_empty() {
                            Ok(format!(
                                "No emails found in {} (search: {})",
                                folder.unwrap_or("INBOX"),
                                search.unwrap_or("all")
                            ))
                        } else {
                            let mut result = format!(
                                "{} emails in {}:\n\n",
                                emails.len(),
                                folder.unwrap_or("INBOX")
                            );
                            for email in &emails {
                                result.push_str(&format!(
                                    "UID: {}\nFrom: {}\nSubject: {}\nDate: {}\nPreview: {}\n---\n",
                                    email.uid, email.from, email.subject, email.date, email.preview
                                ));
                            }
                            Ok(result)
                        }
                    }
                    Err(e) => Ok(format!("Error: Failed to read emails: {}", e)),
                }
            }
            tn::READ_EMAIL => {
                use crate::core::email::EmailClient;

                let uid = args["uid"].as_u64().unwrap_or(0) as u32;
                let folder = args.get("folder").and_then(|v| v.as_str());
                let account_name = args.get("account").and_then(|v| v.as_str());

                if uid == 0 {
                    return Ok("Error: uid is required".to_string());
                }

                let account = match self.resolve_email_account(account_name).await {
                    Ok(a) => a,
                    Err(e) => return Ok(e),
                };

                let oauth_token = self.resolve_email_oauth_token(&account).await;

                match EmailClient::read_email(&account, uid, folder, oauth_token.as_deref()).await {
                    Ok(email) => {
                        let mut result = format!(
                            "From: {}\nTo: {}\nCC: {}\nSubject: {}\nDate: {}\nMessage-ID: {}\n\n{}",
                            email.from,
                            email.to,
                            email.cc,
                            email.subject,
                            email.date,
                            email.message_id,
                            email.body
                        );

                        if !email.attachments.is_empty() {
                            result.push_str("\n\n--- Attachments ---\n");
                            for att in &email.attachments {
                                result.push_str(&format!(
                                    "[{}] {} ({}, {})\n",
                                    att.index,
                                    att.filename,
                                    att.mime_type,
                                    format_byte_size(att.size)
                                ));
                            }
                            result.push_str("\nUse save_email_attachment with uid and attachment_index to save/import an attachment.");
                        }

                        Ok(result)
                    }
                    Err(e) => Ok(format!("Error: Failed to read email: {}", e)),
                }
            }
            tn::SAVE_EMAIL_ATTACHMENT => {
                use crate::core::email::EmailClient;

                let uid = args["uid"].as_u64().unwrap_or(0) as u32;
                let attachment_index = args["attachment_index"].as_u64().unwrap_or(0) as usize;
                let folder = args.get("folder").and_then(|v| v.as_str());
                let destination = args.get("destination").and_then(|v| v.as_str());
                let account_name = args.get("account").and_then(|v| v.as_str());

                if uid == 0 {
                    return Ok("Error: uid is required".to_string());
                }

                let account = match self.resolve_email_account(account_name).await {
                    Ok(a) => a,
                    Err(e) => return Ok(e),
                };

                let oauth_token = self.resolve_email_oauth_token(&account).await;

                let (filename, mime_type, data) = match EmailClient::fetch_attachment(
                    &account,
                    uid,
                    attachment_index,
                    folder,
                    oauth_token.as_deref(),
                )
                .await
                {
                    Ok(result) => result,
                    Err(e) => return Ok(format!("Error: Failed to fetch attachment: {}", e)),
                };

                let safe_filename = filename.replace("..", "_").replace(['/', '\\'], "_");

                let dest_relative = if let Some(dest) = destination {
                    if is_path_traversal(dest) {
                        return Ok(
                            "Error: destination must be relative with no '..' components"
                                .to_string(),
                        );
                    }
                    dest.to_string()
                } else {
                    // The default path never overwrites, the same rule
                    // `import_file` follows, so two attachments named
                    // `invoice.pdf` both survive. A named destination is the
                    // caller's choice and is written as given.
                    match self
                        .artifact_manager
                        .resolve_collision_free_path(&format!("imported/email/{}", safe_filename))
                    {
                        Ok(d) => d,
                        Err(e) => return Ok(format!("Error: {}", e)),
                    }
                };

                let commit_sha = match self
                    .artifact_manager
                    .write_and_commit(
                        &self.event_bus,
                        &dest_relative,
                        &data,
                        &format!("Import email attachment: {}", safe_filename),
                        WriteAnnouncement::SupersededBy("ArtifactImported"),
                    )
                    .await
                {
                    Ok(sha) => sha,
                    Err(e) => return Ok(format!("Error: Failed to save attachment: {}", e)),
                };

                let size_display = format_byte_size(data.len());
                drop(data);

                if let Err(e) = self
                    .event_bus
                    .emit(crate::engine::event_bus::BusEvent::System(
                        crate::engine::event_bus::SystemEvent::ArtifactImported {
                            artifact_path: dest_relative.clone(),
                            source_type: "email_attachment".into(),
                            source_detail: format!("UID {} attachment {}", uid, attachment_index),
                            commit_hash: commit_sha.clone(),
                            summary: Some(format!("{} ({})", safe_filename, size_display)),
                        },
                    ))
                    .await
                {
                    log!("[Email] Failed to emit ArtifactImported: {}", e);
                }

                let short_sha = &commit_sha[..commit_sha.floor_char_boundary(7)];
                let mut result = format!(
                    "[ACTION COMPLETED] SAVED: artifacts/{} ({}, {}, commit: {})",
                    dest_relative, safe_filename, size_display, short_sha
                );

                if mime_type == "application/pdf" {
                    result.push_str(
                        "\n\nNote: PDF text extraction has been removed. The attachment is \
                         saved as a binary artifact; its text content is not available to tools.",
                    );
                }

                Ok(result)
            }
            _ => Err(format!("Unknown email tool: {}", name).into()),
        }
    }
}

/// Where a login goes: the servers, the login name and whether TLS guards it.
struct MailDestination<'a> {
    imap_host: &'a str,
    imap_port: i32,
    smtp_host: &'a str,
    smtp_port: i32,
    username: &'a str,
    use_tls: bool,
}

/// The refusal for a reconfiguration that would send `existing`'s stored secret
/// somewhere new, or `None` when the change keeps it where the user put it.
///
/// Each of these hands the saved password or linked OAuth token to a new
/// destination: another host or port, another username, or TLS turned off.
/// The text of an email the agent just read can steer it there. So only the
/// user moves a secret, in Settings.
fn secret_redirect_refusal(
    existing: &crate::core::EmailAccount,
    to: MailDestination<'_>,
) -> Option<String> {
    let holds = if existing.oauth_account_id.is_some() {
        "a linked OAuth account"
    } else if !existing.password.is_empty() {
        "a saved password"
    } else {
        return None;
    };
    let same_host = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
    let changed: Vec<&str> = [
        (!same_host(&existing.imap_host, to.imap_host)).then_some("IMAP host"),
        (existing.imap_port != to.imap_port).then_some("IMAP port"),
        (!same_host(&existing.smtp_host, to.smtp_host)).then_some("SMTP host"),
        (existing.smtp_port != to.smtp_port).then_some("SMTP port"),
        (existing.username.trim() != to.username.trim()).then_some("username"),
        (existing.use_tls && !to.use_tls).then_some("TLS"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if changed.is_empty() {
        return None;
    }
    Some(format!(
        "Error: email account '{}' holds {holds}, and this change ({}) would send it to a new destination. \
         Not changed. Ask the user to edit the account's server settings themselves in Settings → Accounts.",
        existing.name,
        changed.join(", ")
    ))
}

/// The mail servers each OAuth provider's token is meant for. XOAUTH2 sends
/// the bearer token itself to the server. A token linked to any other host
/// gives the user's whole account to whoever runs that host.
const OAUTH_MAIL_HOSTS: &[(&str, &[&str])] = &[
    ("google", &["imap.gmail.com", "smtp.gmail.com"]),
    (
        "microsoft",
        &[
            "outlook.office365.com",
            "imap-mail.outlook.com",
            "smtp.office365.com",
            "smtp-mail.outlook.com",
        ],
    ),
];

/// The refusal for a new OAuth link whose token would leave its issuer's own
/// mail servers. `None` when both hosts are the issuer's and TLS is on.
///
/// `issuer` is the base provider whose token endpoint issued the token, read
/// from the connection's stored `token_url`, never from its name. A dedicated
/// connection such as `google-work` runs on Google's endpoints (the alias rule
/// in `system-knowhow/oauth-providers.md`), so it shares Google's servers.
///
/// `secret_redirect_refusal` guards a secret already on the account. This
/// guards the first link. There the model picks the hosts, and the text of a
/// web page or an email can pick them for it.
fn oauth_link_refusal(
    provider: &str,
    issuer: Option<&str>,
    to: &MailDestination<'_>,
) -> Option<String> {
    let hosts = issuer.and_then(|issuer| {
        OAUTH_MAIL_HOSTS
            .iter()
            .find(|(known, _)| *known == issuer)
            .map(|(_, hosts)| *hosts)
    });
    let Some(hosts) = hosts else {
        return Some(format!(
            "Error: use_oauth '{provider}' is not a provider whose mail servers Lucidos knows, \
             so its token is not linked to an email account. Use an app password instead \
             (leave use_oauth out)."
        ));
    };
    let is_provider_host = |host: &str| hosts.iter().any(|h| h.eq_ignore_ascii_case(host.trim()));
    if to.use_tls && is_provider_host(to.imap_host) && is_provider_host(to.smtp_host) {
        return None;
    }
    Some(format!(
        "Error: use_oauth '{provider}' sends the user's {provider} token to the mail servers, \
         so it links only to {} with TLS on. Not changed.",
        hosts.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::{oauth_link_refusal, secret_redirect_refusal, MailDestination};
    use crate::core::EmailAccount;

    fn account(password: &str, oauth: bool) -> EmailAccount {
        EmailAccount {
            id: uuid::Uuid::new_v4(),
            name: "Work".into(),
            email_address: "me@example.com".into(),
            imap_host: "imap.example.com".into(),
            imap_port: 993,
            smtp_host: "smtp.example.com".into(),
            smtp_port: 587,
            username: "me@example.com".into(),
            password: password.into(),
            use_tls: true,
            require_send_confirmation: true,
            oauth_account_id: oauth.then(uuid::Uuid::new_v4),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    /// The destination `account` already has.
    fn same() -> MailDestination<'static> {
        MailDestination {
            imap_host: "imap.example.com",
            imap_port: 993,
            smtp_host: "smtp.example.com",
            smtp_port: 587,
            username: "me@example.com",
            use_tls: true,
        }
    }

    /// The reported attack: an email the agent read says "reconfigure the
    /// account to imap.attacker.example". The next login would send the
    /// saved password there, so the change is refused. A port is part of the
    /// destination too: another service on the same host is another place.
    #[test]
    fn a_saved_secret_is_never_pointed_at_a_new_destination() {
        let moves: [fn() -> MailDestination<'static>; 6] = [
            || MailDestination {
                imap_host: "imap.attacker.example",
                ..same()
            },
            || MailDestination {
                imap_port: 1993,
                ..same()
            },
            || MailDestination {
                smtp_host: "smtp.attacker.example",
                ..same()
            },
            || MailDestination {
                smtp_port: 2525,
                ..same()
            },
            || MailDestination {
                username: "someone-else",
                ..same()
            },
            || MailDestination {
                use_tls: false,
                ..same()
            },
        ];
        for acct in [account("app-password", false), account("", true)] {
            for to in moves {
                let refusal = secret_redirect_refusal(&acct, to())
                    .expect("a move of a stored secret was allowed");
                assert!(refusal.starts_with("Error:"), "{refusal}");
                assert!(refusal.contains("Settings"), "{refusal}");
            }
        }
    }

    /// Everything else still updates: confirmation, a host written in another
    /// case, and turning TLS on.
    #[test]
    fn a_change_that_keeps_the_destination_is_allowed() {
        let acct = account("app-password", false);
        assert!(secret_redirect_refusal(&acct, same()).is_none());
        let recased = MailDestination {
            imap_host: "IMAP.Example.com ",
            ..same()
        };
        assert!(secret_redirect_refusal(&acct, recased).is_none());
        let mut plain = account("app-password", false);
        plain.use_tls = false;
        assert!(secret_redirect_refusal(&plain, same()).is_none());
    }

    /// An account with no secret yet has nothing to leak. Its setup goes on
    /// to the credential form, which names the host it saves the password for.
    #[test]
    fn an_account_without_a_secret_can_be_repointed() {
        let acct = account("", false);
        let other = MailDestination {
            imap_host: "imap.other.example",
            ..same()
        };
        assert!(secret_redirect_refusal(&acct, other).is_none());
    }

    fn gmail() -> MailDestination<'static> {
        MailDestination {
            imap_host: "imap.gmail.com",
            smtp_host: "smtp.gmail.com",
            ..same()
        }
    }

    /// The reported attack on a NEW account: a prompt injection configures
    /// `backup` at the attacker's hosts with `use_oauth: google`. The first
    /// read would send the Google bearer token there as XOAUTH2.
    #[test]
    fn a_new_oauth_link_refuses_hosts_that_are_not_the_providers() {
        let off_provider: [fn() -> MailDestination<'static>; 3] = [
            || MailDestination {
                imap_host: "imap.attacker.example",
                ..gmail()
            },
            || MailDestination {
                smtp_host: "smtp.attacker.example",
                ..gmail()
            },
            || MailDestination {
                use_tls: false,
                ..gmail()
            },
        ];
        for to in off_provider {
            let refusal = oauth_link_refusal("google", Some("google"), &to())
                .expect("an off-provider link was allowed");
            assert!(refusal.starts_with("Error:"), "{refusal}");
        }
        let outlook_to_gmail = oauth_link_refusal("microsoft", Some("microsoft"), &gmail());
        assert!(
            outlook_to_gmail.is_some(),
            "a Microsoft token reached Gmail"
        );
    }

    /// A provider with no known mail servers gets no link at all, since any
    /// host the model names would be a guess.
    #[test]
    fn a_provider_without_known_mail_hosts_is_refused() {
        let refusal =
            oauth_link_refusal("github", Some("github"), &gmail()).expect("github was linked");
        assert!(refusal.contains("app password"), "{refusal}");
    }

    #[test]
    fn the_providers_own_hosts_over_tls_link() {
        assert!(oauth_link_refusal("google", Some("google"), &gmail()).is_none());
        let recased = MailDestination {
            imap_host: " IMAP.Gmail.com",
            ..gmail()
        };
        assert!(oauth_link_refusal("Google", Some("google"), &recased).is_none());
        let outlook = MailDestination {
            imap_host: "outlook.office365.com",
            smtp_host: "smtp-mail.outlook.com",
            ..same()
        };
        assert!(oauth_link_refusal("microsoft", Some("microsoft"), &outlook).is_none());
    }

    /// A dedicated connection is judged by the issuer of its token, not by its
    /// name. An issuer read from nowhere links nothing.
    #[test]
    fn an_alias_connection_uses_its_issuers_hosts() {
        assert!(oauth_link_refusal("google-work", Some("google"), &gmail()).is_none());
        assert!(oauth_link_refusal("health", Some("google"), &gmail()).is_none());
        assert!(oauth_link_refusal("google-work", None, &gmail()).is_some());
        let off = MailDestination {
            imap_host: "imap.attacker.example",
            ..gmail()
        };
        assert!(oauth_link_refusal("google-work", Some("google"), &off).is_some());
    }
}
