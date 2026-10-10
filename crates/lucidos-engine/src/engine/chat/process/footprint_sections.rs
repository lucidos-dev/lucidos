//! The section registry of the *workspace prompt footprint* (ADR 0413).
//!
//! One entry per workspace-grown section of a chat turn. Each one calls the
//! builder the turn calls, on data from the turn's own loaders. So the report
//! can never disagree with what a turn sends.

use std::path::{Path, PathBuf};

use crate::core::knowhow::{KnowhowDirs, KnowhowSummary};
use crate::core::response_style::Style;
use crate::core::{App, CredentialInfo, EmailAccountInfo, Intent, OAuthAccountInfo};
use crate::engine::prompt_footprint::{
    app_open_evidence, file_older_than, knowhow_load_evidence, widget_show_evidence, FootprintItem,
    FootprintLimits, FootprintSection, ItemKind, SectionWhen, SystemPromptArea, UseEvidence,
    UseVerdict, WorkspacePromptFootprint,
};
use crate::engine::LucidosEngine;
use crate::llm::provider::ToolDefinition;

use super::workspace_prompt_footprint as footprint;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Everything the sections are built from, loaded the way a turn loads it.
#[derive(Default)]
pub(crate) struct FootprintInputs {
    pub apps: Vec<App>,
    /// The workspace's own reusable widgets. The built-in ones are system
    /// prompt footprint, so they ride `built_in_widgets` instead.
    pub reusable_widgets: Vec<App>,
    pub built_in_widgets: Vec<App>,
    pub knowhow: Vec<KnowhowSummary>,
    pub app_knowhow: Vec<(String, KnowhowSummary)>,
    pub intents: Vec<Intent>,
    pub response_style: Option<Style>,
    pub user_profile: String,
    pub email_accounts: Vec<EmailAccountInfo>,
    pub oauth_accounts: Vec<OAuthAccountInfo>,
    pub stored_credentials: Vec<CredentialInfo>,
    pub builtin_proxies: String,
    pub files: Vec<String>,
    /// Each app's own knowhow docs, by app id.
    pub open_app_knowhow: Vec<(String, Vec<KnowhowSummary>)>,
    pub stopped_mcp_servers: Vec<String>,
    pub mcp_tools: Vec<ToolDefinition>,
    pub mcp_dropped_notice: Option<String>,
}

/// One section as built: the text the turn would carry, and its items.
pub(crate) struct BuiltSection {
    /// `None` for the MCP tool schemas, which ride the tools array and are
    /// costed by `measured_chars` instead.
    pub text: Option<String>,
    measured_chars: usize,
    pub items: Vec<FootprintItem>,
}

impl BuiltSection {
    fn text(text: String, items: Vec<FootprintItem>) -> Self {
        Self {
            text: Some(text),
            measured_chars: 0,
            items,
        }
    }

    /// What the section costs the turn.
    pub fn chars(&self) -> usize {
        match &self.text {
            Some(text) => text.chars().count(),
            None => self.measured_chars,
        }
    }
}

/// One registry entry.
pub(crate) struct SectionSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub when: SectionWhen,
    pub build: fn(&FootprintInputs) -> BuiltSection,
}

/// Every workspace-grown section of a chat turn, in prompt order.
/// `[USER DEVICE & PREFERENCES]` is absent: it is sent whatever the workspace
/// holds, so it is system prompt footprint.
pub(crate) const SECTIONS: &[SectionSpec] = &[
    SectionSpec {
        id: "available-apps",
        title: "Available Apps",
        when: SectionWhen::EveryTurn,
        build: available_apps,
    },
    SectionSpec {
        id: "reusable-widgets",
        title: "Reusable widgets",
        when: SectionWhen::EveryTurn,
        build: reusable_widgets,
    },
    SectionSpec {
        id: "available-intents",
        title: "Available Intents",
        when: SectionWhen::EveryTurn,
        build: available_intents,
    },
    SectionSpec {
        id: "knowhow-routing",
        title: "Know-how routing list",
        when: SectionWhen::EveryTurn,
        build: knowhow_routing,
    },
    SectionSpec {
        id: "response-style",
        title: "Response style",
        when: SectionWhen::EveryTurn,
        build: response_style,
    },
    SectionSpec {
        id: "user-profile",
        title: "User profile",
        when: SectionWhen::EveryTurn,
        build: user_profile,
    },
    SectionSpec {
        id: "email-accounts",
        title: "Email accounts",
        when: SectionWhen::EveryTurn,
        build: email_accounts,
    },
    SectionSpec {
        id: "oauth-accounts",
        title: "OAuth accounts",
        when: SectionWhen::EveryTurn,
        build: oauth_accounts,
    },
    SectionSpec {
        id: "api-credentials",
        title: "API credentials",
        when: SectionWhen::Gated,
        build: api_credentials,
    },
    SectionSpec {
        id: "current-files",
        title: "Current files",
        when: SectionWhen::Gated,
        build: current_files,
    },
    SectionSpec {
        id: "open-app-knowhow",
        title: "Open app know-how",
        when: SectionWhen::OpenApp,
        build: open_app_knowhow,
    },
    SectionSpec {
        id: "stopped-mcp-servers",
        title: "Stopped MCP servers",
        when: SectionWhen::EveryTurn,
        build: stopped_mcp_servers,
    },
    SectionSpec {
        id: "mcp-tool-schemas",
        title: "MCP tool schemas",
        when: SectionWhen::EveryTurn,
        build: mcp_tool_schemas,
    },
    SectionSpec {
        id: "mcp-tools-not-sent",
        title: "MCP tools not sent",
        when: SectionWhen::ToolsDropped,
        build: mcp_tools_not_sent,
    },
];

fn item(kind: ItemKind, id: &str, name: &str, line: &str, clipped_chars: usize) -> FootprintItem {
    FootprintItem {
        kind,
        id: id.to_string(),
        name: name.to_string(),
        chars: line.chars().count(),
        clipped_chars,
        usage: None,
        path: None,
    }
}

/// How much of `written` the shown form leaves out, its ellipsis aside.
fn clipped(written: &str, shown: &str) -> usize {
    if written == shown {
        return 0;
    }
    written
        .chars()
        .count()
        .saturating_sub(shown.trim_end_matches('…').chars().count())
}

fn app_items(apps: &[App], kind: ItemKind) -> Vec<FootprintItem> {
    apps.iter()
        .map(|app| {
            item(
                kind,
                &app.id,
                &app.name,
                &footprint::app_line(app),
                clipped(
                    &app.description,
                    &footprint::app_description(&app.description),
                ),
            )
        })
        .collect()
}

fn available_apps(inputs: &FootprintInputs) -> BuiltSection {
    BuiltSection::text(
        footprint::build_apps_section(&inputs.apps),
        app_items(&inputs.apps, ItemKind::App),
    )
}

/// The lines the workspace's own widgets add to the widgets list. The list's
/// header rides with the built-in widgets every workspace is sent.
fn reusable_widgets(inputs: &FootprintInputs) -> BuiltSection {
    let lines: Vec<(&App, String)> = inputs
        .reusable_widgets
        .iter()
        .map(|w| (w, crate::engine::widget_guidance::widget_line(w)))
        .collect();
    let items = lines
        .iter()
        .map(|(w, line)| {
            item(
                ItemKind::ReusableWidget,
                &w.id,
                &w.name,
                line,
                clipped(&w.description, &footprint::app_description(&w.description)),
            )
        })
        .collect();
    BuiltSection::text(lines.into_iter().map(|(_, line)| line).collect(), items)
}

fn available_intents(inputs: &FootprintInputs) -> BuiltSection {
    let items = inputs
        .intents
        .iter()
        .map(|i| {
            item(
                ItemKind::Intent,
                &i.id,
                &i.name,
                &footprint::intent_line(i),
                0,
            )
        })
        .collect();
    BuiltSection::text(footprint::build_intents_section(&inputs.intents), items)
}

fn knowhow_routing(inputs: &FootprintInputs) -> BuiltSection {
    let clip = |kh: &KnowhowSummary| {
        clipped(
            &kh.description,
            &footprint::routing_description(&kh.description),
        )
    };
    let own = inputs.knowhow.iter().map(|kh| {
        item(
            ItemKind::Knowhow,
            &kh.id,
            &kh.name,
            &footprint::knowhow_line(kh),
            clip(kh),
        )
    });
    let app_scoped = inputs.app_knowhow.iter().map(|(app_id, kh)| {
        item(
            ItemKind::Knowhow,
            &footprint::app_scoped_id(app_id, &kh.id),
            &kh.name,
            &footprint::app_knowhow_routing_line(app_id, kh),
            clip(kh),
        )
    });
    BuiltSection::text(
        footprint::build_knowhow_section(&inputs.knowhow, &inputs.app_knowhow),
        own.chain(app_scoped).collect(),
    )
}

/// Only the user's own instruction: the wrapper, the literacy rules and the
/// floor are engine text.
fn response_style(inputs: &FootprintInputs) -> BuiltSection {
    match &inputs.response_style {
        Some(style) => BuiltSection::text(
            style.instruction.clone(),
            vec![item(
                ItemKind::ResponseStyle,
                &style.id,
                &style.label,
                &style.instruction,
                0,
            )],
        ),
        None => BuiltSection::text(String::new(), Vec::new()),
    }
}

fn user_profile(inputs: &FootprintInputs) -> BuiltSection {
    let items = if inputs.user_profile.is_empty() {
        Vec::new()
    } else {
        vec![item(
            ItemKind::UserProfile,
            "user_profile.md",
            "User profile",
            &inputs.user_profile,
            0,
        )]
    };
    BuiltSection::text(
        footprint::build_user_profile_section(&inputs.user_profile),
        items,
    )
}

fn email_accounts(inputs: &FootprintInputs) -> BuiltSection {
    let items = inputs
        .email_accounts
        .iter()
        .map(|a| {
            item(
                ItemKind::EmailAccount,
                &a.id.to_string(),
                &a.name,
                &footprint::email_account_line(a),
                0,
            )
        })
        .collect();
    BuiltSection::text(
        footprint::build_email_accounts_section(&inputs.email_accounts),
        items,
    )
}

fn oauth_accounts(inputs: &FootprintInputs) -> BuiltSection {
    let items = inputs
        .oauth_accounts
        .iter()
        .map(|a| {
            item(
                ItemKind::OauthAccount,
                &a.id.to_string(),
                &a.provider,
                &footprint::oauth_account_line(a),
                0,
            )
        })
        .collect();
    BuiltSection::text(
        footprint::build_oauth_accounts_section(&inputs.oauth_accounts),
        items,
    )
}

fn api_credentials(inputs: &FootprintInputs) -> BuiltSection {
    let items = inputs
        .stored_credentials
        .iter()
        .map(|c| {
            item(
                ItemKind::Credential,
                &c.service_name,
                &c.service_name,
                &footprint::credential_line(c),
                0,
            )
        })
        .collect();
    BuiltSection::text(
        footprint::build_credentials_section(&inputs.stored_credentials, &inputs.builtin_proxies),
        items,
    )
}

fn current_files(inputs: &FootprintInputs) -> BuiltSection {
    BuiltSection::text(
        footprint::build_file_list_section(&inputs.files),
        Vec::new(),
    )
}

/// The worst turn: the listing of whichever app has the largest. Each app
/// with docs is an item, so the report names the one to trim.
fn open_app_knowhow(inputs: &FootprintInputs) -> BuiltSection {
    let listings: Vec<(&str, String)> = inputs
        .open_app_knowhow
        .iter()
        .map(|(app_id, docs)| {
            (
                app_id.as_str(),
                footprint::build_app_knowhow_listing(app_id, docs),
            )
        })
        .filter(|(_, listing)| !listing.is_empty())
        .collect();
    let name_of = |app_id: &str| {
        inputs
            .apps
            .iter()
            .find(|a| a.id == app_id)
            .map_or(app_id.to_string(), |a| a.name.clone())
    };
    let items = listings
        .iter()
        .map(|(app_id, listing)| item(ItemKind::AppKnowhow, app_id, &name_of(app_id), listing, 0))
        .collect();
    let largest = listings
        .into_iter()
        .map(|(_, listing)| listing)
        .max_by_key(|listing| listing.chars().count())
        .unwrap_or_default();
    BuiltSection::text(largest, items)
}

fn stopped_mcp_servers(inputs: &FootprintInputs) -> BuiltSection {
    let items = inputs
        .stopped_mcp_servers
        .iter()
        .map(|line| item(ItemKind::McpServer, line, line, line, 0))
        .collect();
    BuiltSection::text(
        footprint::build_stopped_mcp_servers_section(&inputs.stopped_mcp_servers),
        items,
    )
}

/// What the running servers' tools cost once the MCP ceiling applies, in the
/// unit the request packer and the MCP page count.
fn mcp_tool_schemas(inputs: &FootprintInputs) -> BuiltSection {
    let cost = crate::engine::context::tool_definitions_chars;
    let items = inputs
        .mcp_tools
        .iter()
        .map(|tool| FootprintItem {
            kind: ItemKind::McpTool,
            id: tool.name.clone(),
            name: tool.name.clone(),
            chars: cost(std::slice::from_ref(tool)),
            clipped_chars: 0,
            usage: None,
            path: None,
        })
        .collect();
    BuiltSection {
        text: None,
        measured_chars: cost(&inputs.mcp_tools),
        items,
    }
}

fn mcp_tools_not_sent(inputs: &FootprintInputs) -> BuiltSection {
    BuiltSection::text(
        inputs.mcp_dropped_notice.clone().unwrap_or_default(),
        Vec::new(),
    )
}

/// Build every registry section from `inputs`, with no use evidence yet.
pub(crate) fn build_sections(
    inputs: &FootprintInputs,
) -> Vec<(&'static SectionSpec, BuiltSection)> {
    SECTIONS
        .iter()
        .map(|spec| (spec, (spec.build)(inputs)))
        .collect()
}

/// The evidence the unused window is judged against, per item kind.
struct Evidence<'a> {
    apps: UseEvidence,
    widgets: UseEvidence,
    knowhow: UseEvidence,
    app_manager: &'a crate::core::AppManager,
    knowhow_dirs: KnowhowDirs,
    data_dir: PathBuf,
    unused_days: u32,
    /// Docs an intent loads without a `load_knowhow` call: its `knowhow:`
    /// ids, and every doc of an app whose intent runs. They count as used.
    intent_knowhow: IntentKnowhow,
}

/// What `execute_intent` loads into an intent's turn, for the unused check.
#[derive(Default)]
struct IntentKnowhow {
    ids: std::collections::HashSet<String>,
    apps: std::collections::HashSet<String>,
}

impl IntentKnowhow {
    fn of(intents: &[Intent]) -> Self {
        Self {
            ids: intents
                .iter()
                .flat_map(|i| i.knowhow.iter().cloned())
                .collect(),
            apps: intents
                .iter()
                .filter_map(|i| i.id.split_once('/').map(|(app, _)| app.to_string()))
                .collect(),
        }
    }

    /// `app_scoped` says the id resolved into an app's own folder. A
    /// workspace group doc can share the `<app>/<rest>` shape, and an app
    /// intent does not load it.
    fn loads(&self, knowhow_id: &str, app_scoped: bool) -> bool {
        self.ids.contains(knowhow_id)
            || (app_scoped
                && knowhow_id
                    .split_once('/')
                    .is_some_and(|(app, _)| self.apps.contains(app)))
    }
}

impl Evidence<'_> {
    /// Judge an app, reusable widget or knowhow item, and give a knowhow doc
    /// the path the user edits it at.
    fn judge(&self, item: &mut FootprintItem) {
        let (evidence, path) = match item.kind {
            ItemKind::App => (&self.apps, Some(self.app_manager.manifest_path(&item.id))),
            ItemKind::ReusableWidget => (
                &self.widgets,
                Some(self.app_manager.manifest_path(&item.id)),
            ),
            ItemKind::Knowhow => (
                &self.knowhow,
                knowhow_doc_path(&self.knowhow_dirs, &item.id),
            ),
            _ => return,
        };
        let older = path
            .as_deref()
            .is_some_and(|p| file_older_than(p, self.unused_days));
        let mut usage = evidence.usage_of(&item.id, older);
        let app_scoped = path.as_deref().is_some_and(|p| {
            self.knowhow_dirs
                .apps
                .as_deref()
                .is_some_and(|apps| p.starts_with(apps))
        });
        if item.kind == ItemKind::Knowhow {
            usage.verdict = match usage.verdict {
                _ if self.intent_knowhow.loads(&item.id, app_scoped) => UseVerdict::Used,
                UseVerdict::Unused => UseVerdict::NotLoadedByName,
                verdict => verdict,
            };
        }
        item.usage = Some(usage);
        if item.kind == ItemKind::Knowhow {
            item.path = path
                .as_deref()
                .and_then(|p| p.strip_prefix(&self.data_dir).ok())
                .map(|rel| rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// The file a routing-list knowhow id names, in the order `load_knowhow`
/// resolves it: the workspace's own, the shared folder, then an app's.
fn knowhow_doc_path(dirs: &KnowhowDirs, id: &str) -> Option<PathBuf> {
    if crate::core::is_path_traversal(id) {
        return None;
    }
    let file = format!("{id}.md");
    let app_scoped = dirs
        .apps
        .as_deref()
        .and_then(|apps| crate::core::knowhow::app_scoped_knowhow_path(apps, id));
    [
        Some(dirs.local.join(&file)),
        dirs.shared
            .as_deref()
            .map(|shared: &Path| shared.join(&file)),
        app_scoped,
    ]
    .into_iter()
    .flatten()
    .find(|path| path.is_file())
}

impl LucidosEngine {
    /// Load what a chat turn's workspace-grown sections are built from, with
    /// the loaders the turn itself calls.
    async fn footprint_inputs(&self, intents: Vec<Intent>) -> Result<FootprintInputs, BoxError> {
        let (apps, widgets) = self.app_manager.apps_and_reusable_widgets()?;
        let (built_in_widgets, reusable_widgets) = widgets.into_iter().partition(|w| w.built_in);
        let (knowhow, app_knowhow) = self.workspace_knowhow_summaries();
        let open_app_knowhow = apps
            .iter()
            .map(|app| {
                let docs = app_knowhow
                    .iter()
                    .filter(|(app_id, _)| *app_id == app.id)
                    .map(|(_, kh)| kh.clone())
                    .collect();
                (app.id.clone(), docs)
            })
            .collect();
        let files = self.artifact_manager.list_artifacts().unwrap_or_else(|e| {
            crate::log!(
                "[Footprint] Cannot list artifacts: {}. Reporting no files",
                e
            );
            Vec::new()
        });
        let (_, window) = self.chat_model_and_window().await;
        let surface = self
            .mcp_manager
            .tool_surface(crate::mcp::mcp_tool_char_ceiling(window))
            .await;
        Ok(FootprintInputs {
            apps,
            reusable_widgets,
            built_in_widgets,
            knowhow,
            app_knowhow,
            intents,
            response_style: crate::core::response_style::user_written_selection(&self.pool).await,
            user_profile: self.user_profile.snapshot().await,
            email_accounts: self.email_accounts_for_prompt().await,
            oauth_accounts: self.oauth_accounts_for_prompt().await,
            stored_credentials: self.stored_credentials_for_prompt().await,
            builtin_proxies: self.builtin_proxies_section().await,
            files,
            open_app_knowhow,
            stopped_mcp_servers: self.mcp_manager.get_stopped_server_summaries().await,
            mcp_dropped_notice: surface.dropped_notice(),
            mcp_tools: surface.tools,
        })
    }

    /// The *workspace prompt footprint* report, beside the system prompt
    /// footprint for scale.
    pub(crate) async fn workspace_prompt_footprint(
        &self,
    ) -> Result<WorkspacePromptFootprint, BoxError> {
        let limits = FootprintLimits::read(&self.pool).await;
        let capabilities = self.read_turn_capabilities().await;
        let intent_knowhow = IntentKnowhow::of(&capabilities.intents);
        let gates = capabilities.gates;
        let inputs = self.footprint_inputs(capabilities.intents).await?;
        let system_prompt_areas = super::system_prompt::system_prompt_footprint(
            &gates,
            &self.system_knowhow_summaries(),
            &inputs.built_in_widgets,
        )
        .into_iter()
        .map(|(label, chars)| SystemPromptArea { label, chars })
        .collect();
        let evidence = Evidence {
            apps: app_open_evidence(&self.pool, limits.unused_days).await?,
            widgets: widget_show_evidence(&self.pool, limits.unused_days).await?,
            knowhow: knowhow_load_evidence(&self.pool, limits.unused_days).await?,
            app_manager: &self.app_manager,
            knowhow_dirs: self.knowhow_dirs(),
            data_dir: self.workspace_path.join(crate::core::DATA_DIR),
            unused_days: limits.unused_days,
            intent_knowhow,
        };
        Ok(assemble(
            build_sections(&inputs),
            &limits,
            |item| evidence.judge(item),
            system_prompt_areas,
        ))
    }
}

/// The report from built sections: ceilings applied, items judged.
fn assemble(
    built: Vec<(&'static SectionSpec, BuiltSection)>,
    limits: &FootprintLimits,
    mut judge: impl FnMut(&mut FootprintItem),
    system_prompt_areas: Vec<SystemPromptArea>,
) -> WorkspacePromptFootprint {
    let sections: Vec<FootprintSection> = built
        .into_iter()
        .map(|(spec, mut section)| {
            section.items.iter_mut().for_each(&mut judge);
            let chars = section.chars();
            FootprintSection {
                id: spec.id,
                title: spec.title,
                when: spec.when,
                chars,
                over_ceiling: chars > limits.section_ceiling,
                items: section.items,
            }
        })
        .collect();
    let total_chars = sections.iter().map(|s| s.chars).sum();
    WorkspacePromptFootprint {
        sections,
        total_chars,
        over_total_ceiling: total_chars > limits.total_ceiling,
        section_ceiling: limits.section_ceiling,
        total_ceiling: limits.total_ceiling,
        unused_days: limits.unused_days,
        system_prompt_chars: system_prompt_areas.iter().map(|a| a.chars).sum(),
        system_prompt_areas,
    }
}

#[cfg(test)]
#[path = "footprint_sections_tests.rs"]
mod tests;
