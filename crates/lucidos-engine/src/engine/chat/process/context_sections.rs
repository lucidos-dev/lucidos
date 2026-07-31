//! Per-turn context-section builders: file list, user profile, credentials,
//! email/oauth accounts, active app/file/url context, and thread-depth
//! guidance. Each is an independent string block consumed by both the user
//! message assembly and the capture sections. Split out of
//! `process_message_with_steps_internal`.

use crate::core::{CredentialStore, EmailStore, OAuthStore};
use crate::engine::types::AppContext;
use crate::engine::LucidosEngine;
use uuid::Uuid;

use super::super::recursion_guard::MAX_THREAD_DEPTH;

/// The nine per-turn context-section strings produced by
/// [`LucidosEngine::build_chat_context_sections`]. Empty strings mean the
/// section does not apply this turn (callers filter them out).
pub(super) struct ChatContextSections {
    pub file_list_context: String,
    pub profile_context: String,
    pub device_preferences_context: String,
    pub credentials_context: String,
    pub email_accounts_context: String,
    pub oauth_context: String,
    pub app_context_section: String,
    pub file_context_section: String,
    pub url_context_section: String,
    pub thread_depth_context: String,
}

/// Borrowed per-turn inputs to [`LucidosEngine::build_chat_context_sections`].
/// Bundles the request context so the builder takes one argument instead of
/// eight (the per-turn counterpart to the [`ChatContextSections`] output).
pub(super) struct ChatContextInputs<'a> {
    pub classification: &'a crate::memory::QueryClassification,
    pub user_profile: &'a str,
    pub device_id: Option<&'a str>,
    pub event_device: Option<&'a str>,
    pub app_context: Option<&'a AppContext>,
    pub file_context: Option<&'a str>,
    pub url_context: Option<&'a crate::api::UrlContext>,
    pub parent_thread_id: Option<Uuid>,
}

impl LucidosEngine {
    /// Build the per-turn context sections. Verbatim extraction of the inline
    /// blocks from `process_message_with_steps_internal`.
    pub(super) async fn build_chat_context_sections(
        &self,
        inputs: ChatContextInputs<'_>,
    ) -> ChatContextSections {
        let ChatContextInputs {
            classification,
            user_profile,
            device_id,
            event_device,
            app_context,
            file_context,
            url_context,
            parent_thread_id,
        } = inputs;
        // Include current file list so LLM doesn't need to call list_files
        let file_list_context = if !classification.needs_file_list {
            log!("[Chat] Skipping file list context (not needed for this query)");
            String::new()
        } else {
            let all_files = self.artifact_manager.list_artifacts().unwrap_or_default();

            if !all_files.is_empty() {
                let file_list = all_files
                    .iter()
                    .take(100)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n  ");
                let suffix = if all_files.len() > 100 {
                    format!("\n  ... and {} more", all_files.len() - 100)
                } else {
                    String::new()
                };
                format!("[CURRENT FILES]\n  {}{}\n[END FILES]", file_list, suffix)
            } else {
                String::new()
            }
        }; // end file_list_context if/else

        // Include user profile content so LLM doesn't need to read it
        let profile_context = if user_profile.is_empty() {
            String::new()
        } else {
            format!("[USER PROFILE - ALREADY IN CONTEXT - NEVER call read_file on user_profile.md]\n{}\n[END PROFILE]", user_profile)
        };

        let device_preferences_context =
            crate::engine::agent_context::build_user_device_preferences_context(
                &self.pool,
                device_id,
                event_device,
            )
            .await;

        // Include available API credentials so LLM knows which services are configured
        let credentials_context = if !classification.needs_credentials {
            log!("[Chat] Skipping credentials context (not needed for this query)");
            String::new()
        } else {
            match CredentialStore::list(&self.pool).await {
                Ok(creds) if !creds.is_empty() => {
                    let cred_list: Vec<String> = creds
                        .iter()
                        .map(|c| format!("  - {} ({})", c.service_name, c.base_url))
                        .collect();
                    format!("[CONFIGURED API CREDENTIALS - auth headers are auto-injected for these services]\n{}\nYou can use http_request directly with these APIs - credentials are automatically added.\n[END CREDENTIALS]", cred_list.join("\n"))
                }
                _ => String::new(),
            }
        };

        // Add configured email accounts to context
        let email_accounts_context = match EmailStore::list(&self.pool).await {
            Ok(accounts) if !accounts.is_empty() => {
                let mut section = String::from("[CONFIGURED EMAIL ACCOUNTS]\n");
                for acc in &accounts {
                    let auth = if acc.oauth_account_id.is_some() {
                        " [OAuth]"
                    } else {
                        ""
                    };
                    section.push_str(&format!("- {} ({}){}\n", acc.name, acc.email_address, auth));
                }
                section.push_str("[END EMAIL ACCOUNTS]");
                section
            }
            _ => String::new(),
        };

        // Add connected OAuth accounts to context
        let oauth_context = match OAuthStore::list(&self.pool).await {
            Ok(accounts) if !accounts.is_empty() => {
                let account_list: Vec<String> = accounts
                    .iter()
                    .map(|a| {
                        let email = a.email.as_deref().unwrap_or("unknown");
                        format!("  - {} ({}) — scopes: {}", a.provider, email, a.scopes)
                    })
                    .collect();
                format!("\n[CONNECTED OAUTH ACCOUNTS - auth tokens are auto-injected and refreshed for these providers]\n{}\nYou can use http_request with these providers' APIs — OAuth tokens are automatically added.\n[END OAUTH ACCOUNTS]", account_list.join("\n"))
            }
            _ => String::new(),
        };

        // Active app context — tell the LLM which app UI is open so it can read files as needed
        let app_context_section = if let Some(ctx) = app_context {
            let app_name = self
                .app_manager
                .get_app(&ctx.app_id)
                .ok()
                .map(|a| a.name.clone())
                .unwrap_or_else(|| ctx.app_id.clone());

            // Load app-specific knowhow from data/apps/{id}/knowhow/*.md
            let app_knowhow_dir = self
                .workspace_path
                .join(crate::core::APPS_DIR)
                .join(&ctx.app_id)
                .join("knowhow");
            let app_knowhow = crate::core::knowhow::load_app_knowhow(&app_knowhow_dir);

            active_app_ui_section(&app_name, &ctx.app_id, &app_knowhow)
        } else {
            String::new()
        };

        // Handle file context when the user is viewing a specific file
        let file_context_section = if let Some(path) = file_context {
            active_file_context_section(path)
        } else {
            String::new()
        };

        // Handle URL context when the user has a webpage open in the panel webview
        let url_context_section = if let Some(ctx) = url_context {
            let title_line = ctx
                .title
                .as_deref()
                .map(|t| format!("Title: {}\n", t))
                .unwrap_or_default();
            if ctx.content.trim().is_empty() {
                // URL is open but content couldn't be extracted (cross-origin, CSP, etc.)
                format!(
                    "[ACTIVE URL CONTEXT]\n\
The user currently has a webpage open in the browser panel.\n\
URL: {}\n\
{}\
Page content could not be extracted. If the user asks about this page, use browse_url or web_search to retrieve its content.\n\
[END URL CONTEXT]",
                    ctx.url, title_line
                )
            } else {
                // Truncate content to 100K chars for safety (frontend already caps, but belt-and-suspenders)
                let content = if ctx.content.len() > 100_000 {
                    &ctx.content[..ctx.content.floor_char_boundary(100_000)]
                } else {
                    &ctx.content
                };
                format!(
                    "[ACTIVE URL CONTEXT]\n\
The user currently has a webpage open in a side panel and may be asking about its content.\n\
URL: {}\n\
{}\
--- Page Content ---\n\
{}\n\
--- End Page Content ---\n\
[END URL CONTEXT]",
                    ctx.url, title_line, content
                )
            }
        } else {
            String::new()
        };

        // Thread depth context — tell child threads their position so they make
        // better decisions about sub-threading
        let thread_depth_context = if let Some(pid) = parent_thread_id {
            let parent_depth: Result<Option<i32>, _> =
                sqlx::query_scalar("SELECT depth FROM thread_summaries WHERE thread_id = $1")
                    .bind(pid)
                    .fetch_optional(&self.pool)
                    .await;
            let depth: i32 = match parent_depth {
                Ok(Some(d)) => d + 1,
                Ok(None) => 1, // parent not yet in projection — default to depth 1
                Err(e) => {
                    log!(
                        "[Chat] Failed to query parent thread depth for context: {}",
                        e
                    );
                    1
                }
            };

            let remaining = MAX_THREAD_DEPTH - depth;
            let guidance = if remaining <= 0 {
                "You are at maximum thread depth. Do ALL work directly — no sub-threading available.".to_string()
            } else if remaining == 1 {
                "You have 1 level of sub-threading remaining. Use it only if absolutely necessary — strongly prefer doing work directly.".to_string()
            } else {
                format!("You have {} levels of sub-threading remaining. Prefer doing work directly; only delegate truly independent parallel tasks.", remaining)
            };
            format!(
                "[THREAD CONTEXT]\n\
                 You are a child thread at depth {} (max {}).\n\
                 {}\n\
                 [END THREAD CONTEXT]",
                depth, MAX_THREAD_DEPTH, guidance
            )
        } else {
            String::new()
        };

        ChatContextSections {
            file_list_context,
            profile_context,
            device_preferences_context,
            credentials_context,
            email_accounts_context,
            oauth_context,
            app_context_section,
            file_context_section,
            url_context_section,
            thread_depth_context,
        }
    }
}

/// Shared anchoring clause appended to the `[ACTIVE APP UI]` and
/// `[ACTIVE FILE CONTEXT]` sections. It leans the chat agent toward treating
/// what the user has OPEN as the default subject of an ambiguous request —
/// without forbidding anything. The motivating failure: a user editing their
/// own `lucidos-me` site (a landing page *about* Lucidos) asked to fix a "logo
/// tooltip on the Conversation & Canvas mark"; the chat agent took the
/// Lucidos-flavoured vocabulary as a cue to go hunt and edit Lucidos's own
/// product source checkout (grepping `crates/lucidos-app/src`, ballooning
/// context past 600k tokens) instead of the open artifact. The clause is a
/// DEFAULT, not a lock: the agent must still do anything the user clearly asks
/// for a different target (another app, Lucidos itself, a workspace task).
const ANCHOR_DEFAULT_RULE: &str = " This is a DEFAULT, not a restriction — you \
can still do anything the user clearly asks (edit another app, work on Lucidos \
the product itself, run a workspace task) when their request names a different \
target. But don't jump to the Lucidos product source for a request that fits \
what's open: words that also name Lucidos's own UI (\"logo\", \"mark\", \
\"Conversation\", \"Canvas\", \"panes\", \"thread\") routinely appear in the \
user's own app or site content — which may itself be about Lucidos — so they \
are NOT by themselves a signal to read or edit Lucidos's source code. When the \
target is genuinely unclear, default to what's open and switch only if the user \
indicates otherwise.";

/// Build the `[ACTIVE APP UI]` context section for an open app. Pure so the
/// prompt text (including [`ANCHOR_DEFAULT_RULE`]) is unit-testable without a
/// `LucidosEngine`/DB.
fn active_app_ui_section(app_name: &str, app_id: &str, app_knowhow: &str) -> String {
    format!(
        "[ACTIVE APP UI]\n\
        The user has the '{name}' app UI open. \
        App files are at data/apps/{id}/. \
        Use list_files and read_file to inspect the app's files as needed.\n\
        DEFAULT TO THIS APP: Treat this app as the default subject of the \
        user's request — especially for UI/copy/visual tweaks (text, wording, \
        a label, a tooltip, a logo, layout, styling). Make the change in this \
        app's files (data/apps/{id}/, or the artifact/site this app \
        manages).{anchor}\n\
        {knowhow}\
        [END ACTIVE APP UI]",
        name = app_name,
        id = app_id,
        anchor = ANCHOR_DEFAULT_RULE,
        knowhow = app_knowhow
    )
}

/// Build the `[ACTIVE FILE CONTEXT]` section for an open file/artifact. Pure
/// (see [`active_app_ui_section`]).
fn active_file_context_section(path: &str) -> String {
    format!(
        "[ACTIVE FILE CONTEXT]\n\
The user currently has the file '{path}' open in a preview window. \
They may be asking about or wanting to modify this file. \
Keep this in mind when interpreting their request.\n\
DEFAULT TO THIS FILE: Treat this file as the default subject of the user's \
request — especially for UI/copy/visual tweaks (text, wording, a label, a \
tooltip, a logo, styling). Edit THIS file.{anchor}\n\
[END FILE CONTEXT]",
        path = path,
        anchor = ANCHOR_DEFAULT_RULE
    )
}

#[cfg(test)]
mod tests {
    use super::{active_app_ui_section, active_file_context_section};

    #[test]
    fn app_ui_section_anchors_on_open_app_without_forbidding_other_work() {
        let section = active_app_ui_section("Site publisher", "site-publisher", "");
        // Identifies the open app + its folder.
        assert!(section.contains("[ACTIVE APP UI]"));
        assert!(section.contains("'Site publisher'"));
        assert!(section.contains("data/apps/site-publisher/"));
        // Leans toward the open app as the default subject.
        assert!(section.contains("DEFAULT TO THIS APP"));
        assert!(section.contains("a tooltip"));
        // Overlapping Lucidos vocabulary is explicitly NOT a cue to edit source.
        assert!(
            section.contains("NOT by themselves a signal to read or edit Lucidos's source code")
        );
        // But it's a default, not a lock — other work stays possible.
        assert!(section.contains("DEFAULT, not a restriction"));
        assert!(section.contains("work on Lucidos the product itself"));
    }

    #[test]
    fn file_context_section_anchors_on_open_file() {
        let section = active_file_context_section("artifacts/web/lucidos-me/index.html");
        assert!(section.contains("[ACTIVE FILE CONTEXT]"));
        assert!(section.contains("artifacts/web/lucidos-me/index.html"));
        assert!(section.contains("DEFAULT TO THIS FILE"));
        assert!(section.contains("Edit THIS file."));
        assert!(section.contains("DEFAULT, not a restriction"));
        assert!(
            section.contains("NOT by themselves a signal to read or edit Lucidos's source code")
        );
    }

    #[test]
    fn app_knowhow_is_appended_inside_the_section() {
        let section = active_app_ui_section("Demo", "demo", "[APP KNOWHOW]\nfoo\n");
        assert!(section.contains("[APP KNOWHOW]\nfoo\n"));
        // Knowhow sits before the closing marker.
        let kh = section.find("[APP KNOWHOW]").unwrap();
        let end = section.find("[END ACTIVE APP UI]").unwrap();
        assert!(kh < end);
    }
}
