use super::super::LucidosEngine;
use super::credentials::credential_request_payload;
use crate::core::format_byte_size;
use crate::core::is_path_traversal;
use crate::core::oauth;
use crate::core::OAuthStore;
use crate::core::WriteAnnouncement;
use crate::llm::tool_names as tn;
use uuid::Uuid;

#[easy_ext::ext(EmailExt)]
pub(crate) impl LucidosEngine {
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

        if account.oauth_account_id.is_none() {
            account
                .password
                .to_sign_in(&account.name)
                .map_err(|reason| format!("Error: {reason}"))?;
        }

        Ok(account)
    }

    async fn resolve_email_oauth_token(
        &self,
        account: &crate::core::EmailAccount,
    ) -> Result<Option<String>, String> {
        oauth::linked_account_token(&self.pool, account.oauth_account_id)
            .await
            .map_err(|e| format!("Error: {e}"))
    }

    async fn oauth_token_issuer(&self, provider: &str) -> Option<&'static str> {
        let service = oauth::client_provider_name(provider);
        let credential =
            match crate::core::CredentialStore::get_oauth_client(&self.pool, &service).await {
                Ok(found) => found?,
                Err(e) => {
                    log_legacy!("[Email] Failed to read the {} OAuth client: {}", service, e);
                    return None;
                }
            };
        let config: serde_json::Value = serde_json::from_str(credential.stored_value()).ok()?;
        oauth::provider_for_url(config["token_url"].as_str()?)
    }

    async fn execute_email_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
        _request_id: Uuid,
        thread_id: Uuid,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match name {
            tn::CONFIGURE_EMAIL => {
                use crate::core::EmailStore;

                let name = args["name"].as_str().unwrap_or("");
                let email_address = args["email_address"].as_str().unwrap_or("");
                let imap_host = args["imap_host"].as_str().unwrap_or("");
                let smtp_host = args["smtp_host"].as_str().unwrap_or("");
                let use_tls = args["use_tls"].as_bool().unwrap_or(true);
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

                let existing = match EmailStore::get(&self.pool, name).await {
                    Ok(found) => found,
                    Err(e) => {
                        return Ok(format!(
                            "Error: could not read the '{}' email account: {}. Not changing it until this read works.",
                            name, e
                        ))
                    }
                };
                let requested_confirm = args["require_send_confirmation"].as_bool();
                let require_confirm =
                    send_confirmation_after_configure(requested_confirm, existing.as_ref());
                let confirm_note = if requested_confirm == Some(false) && require_confirm {
                    " Send confirmation stays on: only the user can turn it off, in the account's settings."
                } else {
                    ""
                };
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

                if let Some(oauth_provider) = use_oauth {
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
                        "Email account '{}' configured with OAuth ({}) for SMTP authentication. No app password needed. Ready to send/receive.{}",
                        name, oauth_provider, confirm_note
                    ));
                }

                if let Some(existing) = existing {
                    if existing.password.is_on_file() || existing.oauth_account_id.is_some() {
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
                            "Email account '{}' updated. {} Ready to send/receive.{}",
                            name, auth_note, confirm_note
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

                let validated =
                    crate::core::email::EmailAttachment::validate_paths(&attachment_paths)
                        .map_err(|e| format!("Error: {}", e))?;

                let account = match self.resolve_email_account(account_name).await {
                    Ok(a) => a,
                    Err(e) => return Ok(e),
                };

                let is_trigger = crate::scheduler::user_tasks::current_trigger_id().is_some();
                if send_needs_confirmation(&account, is_trigger) {
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

                let attachments = crate::core::email::EmailAttachment::read_from_workspace(
                    &self.workspace_path,
                    &attachment_paths,
                )
                .map_err(|e| format!("Error: {}", e))?;

                let oauth_token = match self.resolve_email_oauth_token(&account).await {
                    Ok(token) => token,
                    Err(e) => return Ok(e),
                };

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

                let oauth_token = match self.resolve_email_oauth_token(&account).await {
                    Ok(token) => token,
                    Err(e) => return Ok(e),
                };

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

                let oauth_token = match self.resolve_email_oauth_token(&account).await {
                    Ok(token) => token,
                    Err(e) => return Ok(e),
                };

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
                let writer = crate::core::artifact_writer::ArtifactWriterOrigin::Thread(thread_id)
                    .resolve(self.event_bus.pool())
                    .await?;

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

                let oauth_token = match self.resolve_email_oauth_token(&account).await {
                    Ok(token) => token,
                    Err(e) => return Ok(e),
                };

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

                // INVARIANT: Held from the free-name check through the commit, so two
                // imports of one name cannot both claim it.
                let held = self.lock_workspace_repo().await;
                let dest_relative = if let Some(dest) = destination {
                    if is_path_traversal(dest) {
                        return Ok(
                            "Error: destination must be relative with no '..' components"
                                .to_string(),
                        );
                    }
                    crate::core::artifact_relative_path(dest).to_string()
                } else {
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
                        &held,
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

                drop(held);
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
                            writer_thread_id: Some(thread_id),
                            writer: Some(writer.clone()),
                        },
                    ))
                    .await
                {
                    log_legacy!("[Email] Failed to emit ArtifactImported: {}", e);
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

fn send_confirmation_after_configure(
    requested: Option<bool>,
    existing: Option<&crate::core::EmailAccount>,
) -> bool {
    requested == Some(true) || existing.is_none_or(|a| a.require_send_confirmation)
}

fn send_needs_confirmation(account: &crate::core::EmailAccount, is_trigger: bool) -> bool {
    account.require_send_confirmation && !is_trigger
}

struct MailDestination<'a> {
    imap_host: &'a str,
    imap_port: i32,
    smtp_host: &'a str,
    smtp_port: i32,
    username: &'a str,
    use_tls: bool,
}

fn secret_redirect_refusal(
    existing: &crate::core::EmailAccount,
    to: MailDestination<'_>,
) -> Option<String> {
    let holds = if existing.oauth_account_id.is_some() {
        "a linked OAuth account"
    } else if existing.password.is_on_file() {
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
    use super::{
        oauth_link_refusal, secret_redirect_refusal, send_confirmation_after_configure,
        send_needs_confirmation, MailDestination,
    };
    use crate::core::{EmailAccount, MailPassword};

    #[test]
    fn the_agent_cannot_turn_off_send_confirmation() {
        let seeded = account(saved("app-password"), false);
        let stored = send_confirmation_after_configure(Some(false), Some(&seeded));
        assert!(stored, "a false argument lowered the stored flag");
        let reconfigured = EmailAccount {
            require_send_confirmation: stored,
            ..seeded
        };
        assert!(send_needs_confirmation(&reconfigured, false));
    }

    #[test]
    fn a_new_account_always_starts_with_send_confirmation() {
        for requested in [None, Some(false), Some(true)] {
            assert!(
                send_confirmation_after_configure(requested, None),
                "{requested:?} created an account without confirmation"
            );
        }
    }

    #[test]
    fn the_users_choice_to_skip_confirmation_survives_and_can_be_raised() {
        let off = EmailAccount {
            require_send_confirmation: false,
            ..account(saved("app-password"), false)
        };
        assert!(!send_confirmation_after_configure(None, Some(&off)));
        assert!(!send_confirmation_after_configure(Some(false), Some(&off)));
        assert!(send_confirmation_after_configure(Some(true), Some(&off)));
    }

    #[test]
    fn a_trigger_run_sends_without_the_card() {
        let seeded = account(saved("app-password"), false);
        assert!(!send_needs_confirmation(&seeded, true));
    }

    fn saved(password: &str) -> MailPassword {
        MailPassword::Saved(password.into())
    }

    fn account(password: MailPassword, oauth: bool) -> EmailAccount {
        EmailAccount {
            id: uuid::Uuid::new_v4(),
            name: "Work".into(),
            email_address: "me@example.com".into(),
            imap_host: "imap.example.com".into(),
            imap_port: 993,
            smtp_host: "smtp.example.com".into(),
            smtp_port: 587,
            username: "me@example.com".into(),
            password,
            use_tls: true,
            require_send_confirmation: true,
            oauth_account_id: oauth.then(uuid::Uuid::new_v4),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

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
        let unreadable = MailPassword::Unreadable("1Password is unreachable".into());
        for acct in [
            account(saved("app-password"), false),
            account(unreadable, false),
            account(MailPassword::Missing, true),
        ] {
            for to in moves {
                let refusal = secret_redirect_refusal(&acct, to())
                    .expect("a move of a stored secret was allowed");
                assert!(refusal.starts_with("Error:"), "{refusal}");
                assert!(refusal.contains("Settings"), "{refusal}");
            }
        }
    }

    #[test]
    fn a_change_that_keeps_the_destination_is_allowed() {
        let acct = account(saved("app-password"), false);
        assert!(secret_redirect_refusal(&acct, same()).is_none());
        let recased = MailDestination {
            imap_host: "IMAP.Example.com ",
            ..same()
        };
        assert!(secret_redirect_refusal(&acct, recased).is_none());
        let mut plain = account(saved("app-password"), false);
        plain.use_tls = false;
        assert!(secret_redirect_refusal(&plain, same()).is_none());
    }

    #[test]
    fn an_account_without_a_secret_can_be_repointed() {
        let acct = account(MailPassword::Missing, false);
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

    async fn engine_on(db: &str, workspace: &std::path::Path) -> crate::engine::LucidosEngine {
        crate::engine::LucidosEngine::new(
            workspace.to_path_buf(),
            &crate::test_support::test_db_url(db),
            crate::llm::ProviderBuildContext {
                default_model: "mock".into(),
                model_is_mock: true,
                vertex_project_id: String::new(),
                vertex_location: crate::llm::vertex::location_handle(String::new()),
                vertex_token_cache: None,
                model_registry: Default::default(),
                not_served: Default::default(),
                claude_gateway_access: None,
                chatgpt_plan_access: None,
            },
            std::sync::Arc::new(crate::engine::agent_recovery::EngineAgentHost),
            Box::new(crate::engine::provider_adapters::ProviderAdapters::default()),
        )
        .await
        .unwrap()
    }

    fn configure_args(name: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "email_address": "me@example.com",
            "imap_host": "imap.example.com",
            "smtp_host": "smtp.example.com",
        })
    }

    #[tokio::test]
    async fn an_unconnected_oauth_provider_is_refused_before_the_account_is_written() {
        use super::EmailExt;
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db) = setup_test_db().await;
        let workspace = tempfile::tempdir().unwrap();
        let engine = engine_on(&db, workspace.path()).await;

        let mut args = configure_args("Work Mail");
        args["use_oauth"] = serde_json::json!("google");
        let out = engine
            .execute_email_tool(
                crate::llm::tool_names::CONFIGURE_EMAIL,
                &args,
                uuid::Uuid::new_v4(),
                uuid::Uuid::new_v4(),
            )
            .await
            .unwrap();

        assert!(
            out.starts_with("Error: No OAuth account connected"),
            "{out}"
        );
        assert!(
            crate::core::EmailStore::get(&pool, "Work Mail")
                .await
                .unwrap()
                .is_none(),
            "the account was rewritten before the error was reported"
        );

        engine.pool.close().await;
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn the_password_request_names_the_account_byte_for_byte() {
        use super::EmailExt;
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db) = setup_test_db().await;
        let workspace = tempfile::tempdir().unwrap();
        let engine = engine_on(&db, workspace.path()).await;

        let out = engine
            .execute_email_tool(
                crate::llm::tool_names::CONFIGURE_EMAIL,
                &configure_args("Work Mail"),
                uuid::Uuid::new_v4(),
                uuid::Uuid::new_v4(),
            )
            .await
            .unwrap();

        let json = out
            .strip_prefix(crate::engine::tools::credentials::CREDENTIAL_REQUEST_PREFIX)
            .expect("a credential request");
        let payload: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(payload["service"], "Work Mail");

        engine.pool.close().await;
        pool.close().await;
        teardown_test_db(&db).await;
    }
}
