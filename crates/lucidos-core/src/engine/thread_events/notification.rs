use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Tap {
    #[default]
    Modal,
    Navigate {
        to: Box<NavigateUi>,
    },
    /// INVARIANT: Opens normal restore admission and confirmation; never executes a restore.
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

// INVARIANT: Keep retired `{"kind":"none"}` objects readable as `Modal`.
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct NavigateUi {
    pub target: NavigateTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_view: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fragment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
    /// INVARIANT: The thread or trigger to open. A thread's is settled to a uuid at the
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
    /// INVARIANT: The coding agent backend a `NewChat` draft preselects, as
    /// `CodingAgent::as_str` spells it. A string, not the enum: the page
    /// validates it, so an unknown value warns instead of failing the tap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coding_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

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

pub const UNWATCH_PUSH_ACTION: &str = "unwatch";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PushGroup {
    UserOnlyMessages {
        thread_id: Uuid,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        to_driver: bool,
    },
}

impl PushGroup {
    pub fn tag(&self) -> String {
        match self {
            Self::UserOnlyMessages { thread_id, .. } => format!("user-{thread_id}"),
        }
    }

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

    pub fn tap(self) -> Tap {
        Tap::Navigate {
            to: Box::new(NavigateUi {
                target: NavigateTarget::Settings,
                settings_view: Some(self.view.to_string()),
                ..Default::default()
            }),
        }
    }

    pub fn link(self) -> String {
        format!("[{}](settings:{})", self.path, self.view)
    }
}
