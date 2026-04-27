use super::super::document::safe_extract_pdf_text;
use super::super::LucidosEngine;
use crate::api::is_path_traversal;
use crate::core::format_byte_size;
use crate::core::oauth;
use crate::core::OAuthStore;
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
                let imap_port = args["imap_port"].as_i64().unwrap_or(993) as i32;
                let smtp_host = args["smtp_host"].as_str().unwrap_or("");
                let smtp_port = args["smtp_port"].as_i64().unwrap_or(587) as i32;
                let username = args["username"].as_str().unwrap_or(email_address);
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

                // If use_oauth is set, find the matching OAuth account and link it
                if let Some(oauth_provider) = use_oauth {
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

                    match OAuthStore::get_by_provider(&self.pool, oauth_provider).await {
                        Ok(Some(oauth_account)) => {
                            EmailStore::link_oauth(&self.pool, name, Some(oauth_account.id))
                                .await
                                .map_err(|e| format!("Error: {}", e))?;
                            return Ok(format!(
                                "Email account '{}' configured with OAuth ({}) for SMTP authentication. No app password needed. Ready to send/receive.",
                                name, oauth_provider
                            ));
                        }
                        Ok(None) => {
                            return Ok(format!(
                                "Error: No OAuth account connected for provider '{}'. Use connect_oauth_account first.",
                                oauth_provider
                            ));
                        }
                        Err(e) => {
                            return Ok(format!("Error: Failed to look up OAuth account: {}", e))
                        }
                    }
                }

                // Check if account already exists with a password or OAuth link
                if let Ok(Some(existing)) = EmailStore::get(&self.pool, name).await {
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

                let prompt = format!("Enter the app password for {}", email_address);
                let payload = serde_json::json!({
                    "service": format!("email:{}", name),
                    "prompt": prompt,
                    "base_url": format!("smtp://{}", smtp_host),
                    "auth_type": "email_password",
                });
                Ok(format!("[CREDENTIAL_REQUEST]{}", payload))
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

                // Trigger runs are unattended — skip send confirmation.
                // ACTIVE_TRIGGER_ID is set for all trigger executions.
                let is_trigger = crate::scheduler::user_tasks::ACTIVE_TRIGGER_ID
                    .try_with(|_| ())
                    .is_ok();
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
                    format!("imported/email/{}", safe_filename)
                };

                let commit_sha = match self
                    .artifact_manager
                    .write_and_commit(
                        &dest_relative,
                        &data,
                        &format!("Import email attachment: {}", safe_filename),
                    )
                    .await
                {
                    Ok(sha) => sha,
                    Err(e) => return Ok(format!("Error: Failed to save attachment: {}", e)),
                };

                let size_display = format_byte_size(data.len());
                drop(data);

                let pdf_text = if mime_type == "application/pdf" {
                    let full_path = self
                        .workspace_path
                        .join("data/artifacts")
                        .join(&dest_relative);
                    match safe_extract_pdf_text(&full_path) {
                        Ok(text) if !text.trim().is_empty() => {
                            let text_path = format!("{}.txt", dest_relative);
                            if let Err(e) = self
                                .artifact_manager
                                .write_and_commit(
                                    &text_path,
                                    &text,
                                    &format!("Extract text from {}", safe_filename),
                                )
                                .await
                            {
                                log!("[Email] Failed to write extracted text: {}", e);
                            }
                            Some(text)
                        }
                        _ => None,
                    }
                } else {
                    None
                };

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

                if let Some(text) = pdf_text {
                    let total_chars = text.chars().count();
                    let preview: String = text.chars().take(total_chars.min(2000)).collect();
                    result.push_str(&format!(
                        "\n\n--- Extracted PDF Text ({} chars total) ---\n{}",
                        total_chars, preview
                    ));
                    if total_chars > 2000 {
                        result.push_str("\n... (truncated, full text saved as sidecar .txt file)");
                    }
                }

                Ok(result)
            }
            _ => Ok(format!("Unknown email tool: {}", name)),
        }
    }
}
