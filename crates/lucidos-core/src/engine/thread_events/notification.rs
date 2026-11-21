use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Destination for a tap on an OS push or in-app toast.
/// `Modal` opens the inbox. `Navigate` shares the `navigate_ui` target router.
/// `RestoreSnapshot` requests the normal target admission and confirmation flow.
///
/// Wire shape uses a `kind` field, such as `{"kind":"modal"}`.
/// The page-side router validates destination fields such as `app_id`.
/// Producers settle thread ids through `resolve_thread_tap_id` before writing.
///
/// Serde rejects legacy bare strings, surfacing an HTTP 400 or tool error.
/// Historical `{"kind":"none"}` objects read back as `Modal`.
/// The JSONB migration built the projection from old events once.
/// Historical events stay immutable. Rebuilding a pre-migration projection
/// requires the fix in `system-knowhow/workspace-audit/triggers.md`.
///
/// Box the destination because `NavigateUi` has a field per target.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Tap {
    #[default]
    Modal,
    Navigate {
        to: Box<NavigateUi>,
    },
    /// Opens normal restore admission and confirmation; never executes a restore.
    #[serde(rename = "restore-snapshot")]
    RestoreSnapshot {
        workspace_id: String,
        handle: crate::core::snapshot::handle::Handle,
    },
    /// Opens a confirmation to undo the config change `event_id` recorded
    /// (ADR 0623); never undoes on its own.
    #[serde(rename = "undo-config-change")]
    UndoConfigChange {
        event_id: uuid::Uuid,
    },
}

// Keep retired `{"kind":"none"}` objects readable as `Modal`.
// Immutable events and replayed notification rows can still carry them.
// This parser is permanent old-data tolerance.
//
// It delegates to a private derived helper that keeps serde's full strictness:
// legacy bare strings ("modal"/"none"/"open_thread") and unknown object kinds are
// still rejected loudly (the strict-tap guard). ONLY the well-formed
// `{"kind":"none"}` object is coerced — to `Modal`, which then re-serializes as
// `{"kind":"modal"}`, so a rebuilt row never re-emits `none`.
impl<'de> Deserialize<'de> for Tap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum TapWire {
            Modal,
            None,
            Navigate {
                to: Box<NavigateUi>,
            },
            #[serde(rename = "restore-snapshot")]
            RestoreSnapshot {
                workspace_id: String,
                handle: crate::core::snapshot::handle::Handle,
            },
            #[serde(rename = "undo-config-change")]
            UndoConfigChange {
                event_id: uuid::Uuid,
            },
        }
        Ok(match TapWire::deserialize(deserializer)? {
            TapWire::Modal | TapWire::None => Tap::Modal,
            TapWire::Navigate { to } => Tap::Navigate { to },
            TapWire::RestoreSnapshot {
                workspace_id,
                handle,
            } => Tap::RestoreSnapshot {
                workspace_id,
                handle,
            },
            TapWire::UndoConfigChange { event_id } => Tap::UndoConfigChange { event_id },
        })
    }
}

/// Navigation payload — mirrors the `navigate_ui` LLM tool arg shape. Used
/// as the `to` of `Tap::Navigate`. Required sub-fields are not enforced at
/// the Rust layer; the page-side router rejects malformed nav targets.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct NavigateUi {
    pub target: NavigateTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_view: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// The place INSIDE the target to open. With `target: App` it is the app
    /// iframe's `location.hash`. The page-side router hands it over and never
    /// inspects it, since only the app knows what its own targets are. An app
    /// that ignores the hash still opens, exactly as a file opens at the top
    /// when its cited line has gone stale. With `target: Settings` it names a
    /// row on the `settings_view` page, and the page toasts one it cannot find.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fragment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// First line to select in the file preview, 1-based.
    /// For a file target, the router opens the file even if the line is stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// Last line of the selected range, 1-based and INCLUSIVE. Omit for a
    /// single line. Only meaningful alongside `line`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
    /// The thread or trigger to open. A thread's is settled to a uuid at the
    /// producer by `resolve_thread_tap_id`. The page dereferences it later,
    /// with no way to ask what was meant. Decoding stays permissive, so a row
    /// written before that guard is still readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// The coding agent backend a `NewChat` draft preselects, as
    /// `CodingAgent::as_str` spells it. A string, not the enum: the page
    /// validates it, so an unknown value warns instead of failing the tap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coding_agent: Option<String>,
    /// The coding target a `NewChat` draft preselects: a registered
    /// repository name or id, or `data/apps/<id>`. The page resolves it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

/// Navigation target enum — every UI surface a notification tap can deep-link
/// to. Wire format: kebab-case (e.g. `"new-app"`). Mirrors the `navigate_ui`
/// LLM tool's `target` enum.
///
/// Defaults to `Thread` for struct update syntax.
/// The router shows a toast when a required destination field is missing.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum NavigateTarget {
    Files,
    Apps,
    AppStore,
    Plugins,
    Triggers,
    ThreadQueue,
    Changes,
    Notifications,
    Settings,
    App,
    File,
    Trigger,
    #[default]
    Thread,
    NewApp,
    NewTrigger,
    NewChat,
    Url,
}

/// The OS push action that stops watching a root thread. `sw.js` carries a
/// copy, pinned by a test because a service worker served raw cannot import it.
pub const UNWATCH_PUSH_ACTION: &str = "unwatch";

/// Notifications whose OS pushes share one tag, so a burst replaces one banner
/// rather than stacking (ADR 0649). Stored on the notification row, because
/// the wake push rebuilds from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PushGroup {
    /// User-only messages in one thread. The push offers Unwatch, except to
    /// the thread's driver, who cannot unwatch it.
    UserOnlyMessages {
        thread_id: Uuid,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        to_driver: bool,
    },
}

impl PushGroup {
    /// The push tag every notification in the group carries.
    pub fn tag(&self) -> String {
        match self {
            Self::UserOnlyMessages { thread_id, .. } => format!("user-{thread_id}"),
        }
    }

    /// Stamp the group onto a push envelope: its tag, and for user-only
    /// messages to a watcher an Unwatch action with the thread it leaves.
    pub fn apply(&self, payload: &mut serde_json::Value) {
        let notification = &mut payload["notification"];
        notification["tag"] = serde_json::Value::String(self.tag());
        match self {
            Self::UserOnlyMessages {
                to_driver: true, ..
            } => {}
            Self::UserOnlyMessages {
                thread_id,
                to_driver: false,
            } => {
                notification["actions"] = serde_json::json!([
                    { "action": UNWATCH_PUSH_ACTION, "title": "Unwatch" }
                ]);
                notification["data"]["unwatch_thread_id"] =
                    serde_json::Value::String(thread_id.to_string());
            }
        }
    }
}

/// One Settings page an engine notification can send the reader to: the route
/// written the way the UI's breadcrumbs read, and the view id that opens it.
///
/// Holding both in one value is what keeps a notification's body and its tap on
/// the same page. A System subpanel reads "Settings → System → X", not
/// "Settings → X".
///
/// `view` must be one of `NAVIGABLE_SETTINGS_VIEWS` (`llm/tools/misc.rs`), the
/// set the frontend router renders. Anything else toasts "Unknown settings
/// section" instead of navigating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsPage {
    pub path: &'static str,
    pub view: &'static str,
}

impl SettingsPage {
    pub const ACCESS: Self = Self {
        path: "Settings → Access",
        view: "access",
    };
    pub const ACCOUNTS: Self = Self {
        path: "Settings → Accounts",
        view: "accounts",
    };
    pub const BACKUP: Self = Self {
        path: "Settings → System → Backup",
        view: "backup",
    };
    pub const DISK_USAGE: Self = Self {
        path: "Settings → System → Disk Usage",
        view: "disk-usage",
    };
    pub const PROVIDERS: Self = Self {
        path: "Settings → Providers",
        view: "providers",
    };
    pub const MEMORY: Self = Self {
        path: "Settings → System → Memory",
        view: "memory",
    };
    pub const PERMISSIONS: Self = Self {
        path: "Settings → Permissions",
        view: "permissions",
    };

    /// Every page above, for the test that holds each to a renderable view.
    #[cfg(any(test, feature = "test-support"))]
    pub const ALL: [Self; 7] = [
        Self::ACCESS,
        Self::ACCOUNTS,
        Self::BACKUP,
        Self::DISK_USAGE,
        Self::MEMORY,
        Self::PERMISSIONS,
        Self::PROVIDERS,
    ];

    /// A tap that deep-links here, the same way the LLM's `navigate_ui` does.
    pub fn tap(self) -> Tap {
        Tap::Navigate {
            to: Box::new(NavigateUi {
                target: NavigateTarget::Settings,
                settings_view: Some(self.view.to_string()),
                ..Default::default()
            }),
        }
    }

    /// The route as a markdown link to this page, for a notification body.
    ///
    /// The label is the route itself, so every plain-text surface (OS banner,
    /// toast, inbox row) still reads the route after markdown is stripped.
    pub fn link(self) -> String {
        format!("[{}](settings:{})", self.path, self.view)
    }
}
