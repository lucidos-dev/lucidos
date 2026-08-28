//! The registry of **announced surfaces**: every place the engine stores
//! user-visible state, and how a mutation of it reaches everything else.
//!
//! ## Why this exists
//!
//! The `manage_repositories` agent tool once wrote a `repositories` row and
//! emitted nothing. The durable `repo_names` projection never got an entry and
//! the SSE arm that reloads every client's repository list never fired, so an
//! agent-registered repo stayed invisible until a page reload. The row write and
//! its announcement had drifted apart because the announcement lived at the call
//! site, and the new call site forgot it.
//!
//! Adding the emit to that one tool was not the fix. The fix was moving the emit
//! *into the write path* so no caller can skip it, and this registry is what
//! keeps that true for every other surface: it names each one, says how its
//! mutations are announced, and is checked by the tests in
//! `announced_surfaces_tests.rs`. A new table, or a raw write in a module that
//! does not own one, fails a test instead of shipping.
//!
//! ## The three classifications
//!
//! - [`Announcement::Announced`] carries the real guarantee. The owning module
//!   emits, and every mutator a caller can reach emits with it, so the write and
//!   the announcement are one operation.
//! - [`Announcement::Projection`] is the downstream case. The table materializes
//!   an event stream that was already announced upstream, so its writes are the
//!   *result* of an announcement rather than something needing one.
//! - [`Announcement::Silent`] is engine-internal state nothing observes, with the
//!   reason stated inline so the next person re-decides instead of
//!   re-discovering.
//!
//! The guarantee is about **reachability**, not atomicity: the row commits in
//! its own transaction and the emit follows through `emit_or_log`, the same
//! fire-and-forget contract every `SystemEvent` emitter in the engine uses. See
//! `RepositoryStore`'s type doc for what that costs and why it is the right
//! trade.
//!
//! ## Migrations are outside this, deliberately
//!
//! The scan reads `.rs` only, so a `.sql` migration that writes an announced
//! table is invisible to it. As of 2026-08-01, eleven do: seeding the builtin
//! models, disabling superseded ones, renaming a preference key, repairing
//! OAuth credential auth types.
//!
//! That is safe, and not merely tolerated. `sqlx::migrate!()` runs inside
//! `LucidosEngine::new` **before the `EventBus` is constructed** (both are in
//! `engine_impl/construction.rs`, migrator first) and long before the API
//! serves, so there is no bus to emit on and no client to tell. Every consumer
//! reads the post-migration state on its first load. A backup restore does not
//! weaken this: it deliberately runs NO migrations, leaving them to the engine
//! the gateway spawns afterwards (`core::backup::restore_archive_into`).
//!
//! Stated here rather than left implicit, because an unstated scope limit reads
//! as a stronger guarantee than the tests actually give.

/// A reachable writer that deliberately does not announce, with the reason. Kept
/// per surface rather than as a global list so the justification sits next to
/// the surface it applies to.
///
/// **Announcing is the default, and this is the narrow exit from it.** Reach for
/// it only when emitting would be WRONG, never when it is merely inconvenient.
/// "No variant fits this yet" is a reason to ADD one, not to skip the event. A
/// new variant costs five match arms and a row in `.claude/rules/db.md`. A
/// state change that reaches nothing stays invisible to the timeline, to SSE
/// and to every trigger, permanently.
///
/// Every current entry is one of three shapes. The write is downstream of an
/// emit that already happened. It is a cascade the parent event already covers.
/// Or it records something the engine observed, not a decision a caller made.
pub struct ExemptWriter {
    /// The function name as it appears in the owning source file.
    pub function: &'static str,
    /// Why reaching this writer without an emit is correct. State what the
    /// write records and why every candidate event would misdescribe it.
    pub why: &'static str,
}

/// How mutations of a surface reach the rest of the system.
pub enum Announcement {
    /// The owning module emits these `SystemEvent`s from inside its write path.
    /// Every writer a caller outside the module can reach must emit, except the
    /// listed exemptions.
    Announced {
        events: &'static [&'static str],
        exempt: &'static [ExemptWriter],
    },
    /// The table materializes an already-announced event stream. Its writes are
    /// downstream of the announcement, not a state change needing one.
    Projection {
        /// What it is a projection of, for the reader.
        of: &'static str,
    },
    /// Deliberately unannounced engine-internal state.
    Silent { reason: &'static str },
}

/// A Postgres table and the source files allowed to write it.
pub struct TableRule {
    pub table: &'static str,
    /// Source paths relative to `crates/lucidos-engine/src`. A raw
    /// `INSERT INTO` / `UPDATE` / `DELETE FROM` for this table anywhere else is
    /// a test failure: a private writer is worthless if another module can write
    /// the same table behind its back.
    ///
    /// Empty means **nothing writes this table**, which the ownership scan then
    /// enforces as written. Only legal for a non-`Announced` surface: an
    /// announced one with no writer would be announcing nothing.
    pub owners: &'static [&'static str],
    pub announcement: Announcement,
}

/// A module that mutates the workspace `data/` tree, and how it announces.
/// The `data/` equivalent of [`TableRule`]: there is no table to key on, so the
/// unit is the module that owns the write.
pub struct DataWriterRule {
    /// Source path relative to `crates/lucidos-engine/src`.
    pub owner: &'static str,
    /// What the module writes, for the reader.
    pub writes: &'static str,
    pub announcement: Announcement,
}

/// Tables the engine creates at RUNTIME rather than in a migration, with the
/// reason. They still need a [`TABLES`] entry, but a migrated-but-unbooted
/// database will not have them, so the completeness check does not read their
/// absence as a stale registry entry.
pub const RUNTIME_CREATED_TABLES: &[(&str, &str)] = &[(
    "memory_entries",
    "the embedding column's dimension depends on the configured embedding \
     model, which is only known once the engine boots, so PgVectorStore \
     creates the table on first use instead of a migration fixing the width",
)];

/// Every table in the schema. Checked for completeness against
/// `information_schema.tables`, so a new migration cannot add an unclassified
/// table.
pub const TABLES: &[TableRule] = &[
    TableRule {
        table: "apply_all_batches",
        owners: &["engine/apply_all_driver.rs"],
        announcement: Announcement::Announced {
            events: &["ApplyAllBatchStarted", "ApplyAllBatchCompleted"],
            exempt: &[],
        },
    },
    TableRule {
        table: "browser_logins",
        owners: &["runtime/browser/mod.rs"],
        announcement: Announcement::Silent {
            reason: "A record of which domains the headless browser has an \
                     authenticated session for. Engine-internal: no list \
                     surface, no projection, and nothing reloads on a change.",
        },
    },
    TableRule {
        table: "changes",
        owners: &["core/changes_projection.rs"],
        announcement: Announcement::Projection {
            of: "the change-lifecycle thread events, written through by \
                 event_bus_projection_thread in the same transaction as the \
                 thread_summaries update",
        },
    },
    TableRule {
        table: "credentials",
        owners: &["core/credentials.rs"],
        announcement: Announcement::Announced {
            events: &[
                "CredentialCreated",
                "CredentialUpdated",
                "CredentialDeleted",
            ],
            exempt: &[],
        },
    },
    TableRule {
        table: "device_presence",
        owners: &["core/device_presence.rs"],
        announcement: Announcement::Projection {
            of: "DeviceVisible / DeviceHidden, applied by the EventBus as it \
                 handles them",
        },
    },
    TableRule {
        table: "devices",
        owners: &["core/devices.rs"],
        announcement: Announcement::Announced {
            events: &[
                "DeviceRegistered",
                "DeviceRenamed",
                "DevicePushChanged",
                "DeviceDeleted",
                "DeviceHandedOver",
            ],
            exempt: &[],
        },
    },
    TableRule {
        table: "email_accounts",
        owners: &["core/email.rs"],
        announcement: Announcement::Silent {
            reason: "No projection, no SSE consumer and no list route. An email \
                     account becomes user-visible only once its paired \
                     credential lands, and that write does emit \
                     CredentialCreated. Give this table its own events when a \
                     surface starts listing accounts directly.",
        },
    },
    TableRule {
        table: "environment_variables",
        owners: &["core/environment_variables.rs"],
        announcement: Announcement::Announced {
            events: &["EnvironmentVariableSet", "EnvironmentVariableDeleted"],
            exempt: &[],
        },
    },
    TableRule {
        table: "events",
        owners: &["core/image_migration.rs", "engine/event_bus/mod.rs"],
        announcement: Announcement::Silent {
            reason: "The event log itself. EventBus::persist is the append path \
                     and is private; announcing an append would be circular. \
                     image_migration rewrites historical payloads in place, \
                     which is a data migration rather than a state change.",
        },
    },
    TableRule {
        table: "hardened_branches",
        owners: &["engine/git_ops/harden_marker.rs"],
        announcement: Announcement::Silent {
            reason: "The per-branch hardening marker Apply reads to decide \
                     whether to run /harden synchronously. Build-gate \
                     bookkeeping consumed by one caller, never listed.",
        },
    },
    TableRule {
        table: "headless_blocked",
        owners: &["runtime/browser/mod.rs"],
        announcement: Announcement::Silent {
            reason: "Domains the headless browser is blocked from. \
                     Engine-internal, same shape as browser_logins.",
        },
    },
    TableRule {
        table: "mcp_servers",
        owners: &["core/mcp_servers.rs"],
        announcement: Announcement::Announced {
            events: &[
                "McpServerRegistered",
                "McpServerUpdated",
                "McpServerDisabledToolsChanged",
                "McpServerRemoved",
            ],
            exempt: &[ExemptWriter {
                function: "set_tools",
                why: "Caches the tool manifest a live server just advertised, \
                      which is an observation rather than a decision anyone \
                      made. The only event that could carry it, \
                      McpServerUpdated, is documented as an auto-approve \
                      change, so emitting it would put a permission change \
                      nobody made on the timeline. The row's \
                      tools_observed_at stamp is the signal instead, and the \
                      MCP settings page reads it.",
            }],
        },
    },
    TableRule {
        table: "memory_entries",
        owners: &["engine/memory/rebuild.rs", "memory/pgvector.rs"],
        announcement: Announcement::Silent {
            reason: "The derived vector index. Entries are rebuilt from the \
                     events and artifacts that were themselves announced; \
                     MemoryRebuildProgress covers the only user-visible \
                     operation on it.",
        },
    },
    TableRule {
        table: "models",
        owners: &["core/models.rs"],
        announcement: Announcement::Announced {
            events: &["ModelCreated", "ModelUpdated", "ModelDeleted"],
            exempt: &[],
        },
    },
    TableRule {
        table: "notifications",
        owners: &[
            "engine/event_bus_projection_system.rs",
            "scheduler/notifications.rs",
        ],
        announcement: Announcement::Announced {
            events: &[
                "NotificationCreated",
                "NotificationRead",
                "NotificationsAllRead",
            ],
            exempt: &[
                ExemptWriter {
                    function: "update_system_projection",
                    why: "Materializes the NotificationCreated the emitter \
                          already announced. Emitting again from the projection \
                          would loop.",
                },
                ExemptWriter {
                    function: "insert_with_timestamp",
                    why: "Backdated insert reached only by the populate_memory \
                          dev seeding binary, which fabricates history rather \
                          than reporting a live change.",
                },
                ExemptWriter {
                    function: "insert",
                    why: "Thin wrapper over insert_with_timestamp, same caller.",
                },
            ],
        },
    },
    TableRule {
        table: "oauth_accounts",
        owners: &["core/oauth.rs"],
        announcement: Announcement::Announced {
            events: &["OAuthAccountConnected", "OAuthAccountDeleted"],
            exempt: &[ExemptWriter {
                function: "update_tokens",
                why: "A token rotation refreshes an existing account in place. \
                      Nothing user-visible changed, and announcing every \
                      refresh would put an events row on the timeline each time \
                      a token neared expiry.",
            }],
        },
    },
    TableRule {
        table: "pinned_apps",
        owners: &["core/pinned_apps.rs"],
        announcement: Announcement::Announced {
            events: &["PinnedAppPinned", "PinnedAppUnpinned"],
            exempt: &[
                ExemptWriter {
                    function: "delete_for_device",
                    why: "The cascade from DeviceStore::delete. The device is gone, \
                          and DeviceDeleted already tells every client to drop it; \
                          an unpin event per pinned app would be noise about a \
                          device that no longer exists.",
                },
                ExemptWriter {
                    function: "move_device",
                    why: "The cascade from DeviceStore::hand_over, which announces \
                          DeviceHandedOver. The pins themselves are unchanged; only \
                          the device id naming them moved, so a pin event per app \
                          would report a change nobody made.",
                },
            ],
        },
    },
    TableRule {
        table: "planned_branches",
        owners: &["engine/git_ops/plan_marker.rs"],
        announcement: Announcement::Silent {
            reason: "The per-branch plan marker the edit gate and Apply read. \
                     Build-gate bookkeeping, same shape as hardened_branches.",
        },
    },
    TableRule {
        table: "preferences",
        owners: &["core/devices.rs", "core/preferences.rs"],
        announcement: Announcement::Announced {
            events: &["PreferencesChanged"],
            exempt: &[ExemptWriter {
                function: "set_silent",
                why: "The guarded silent door for engine-internal keys \
                      (SILENT_PREF_KEYS). It rejects any key not on that list, \
                      so it cannot be used to write a user-visible preference \
                      quietly.",
            }],
        },
    },
    TableRule {
        table: "push_log",
        owners: &["scheduler/push_test_log.rs"],
        announcement: Announcement::Silent {
            reason: "A diagnostic log of push delivery attempts, read only when \
                     debugging notification fan-out.",
        },
    },
    TableRule {
        table: "push_subscriptions",
        owners: &["core/devices.rs", "scheduler/push.rs"],
        announcement: Announcement::Silent {
            reason: "The browser-issued Web Push endpoint for a device. The \
                     user-visible half of that state is devices.push_enabled, \
                     which does announce via DevicePushChanged; the endpoint \
                     itself is transport plumbing the browser rotates on its \
                     own.",
        },
    },
    TableRule {
        table: "repo_names",
        owners: &[
            "core/store/threads/backfill.rs",
            "engine/event_bus_projection_system.rs",
        ],
        announcement: Announcement::Projection {
            of: "RepositoryAdded, so a thread bound to a repo still resolves a \
                 label after the repo is unregistered",
        },
    },
    TableRule {
        table: "repositories",
        owners: &["core/repositories.rs"],
        announcement: Announcement::Announced {
            events: &["RepositoryAdded", "RepositoryRemoved"],
            exempt: &[],
        },
    },
    TableRule {
        table: "trigger_crons",
        owners: &[],
        announcement: Announcement::Silent {
            reason: "The pre-event-sourcing trigger table, kept alive only by \
                     the migration chain that renamed scheduled_tasks to it. \
                     The scheduler reads it once at startup, replays each row \
                     as a TriggerCreated event, then DROPs it, so a running \
                     workspace has no such table and nothing ever writes one. \
                     Announcing happens on the replayed events.",
        },
    },
    TableRule {
        table: "thread_queue",
        owners: &["engine/event_bus_projection_system.rs"],
        announcement: Announcement::Projection {
            of: "the ThreadQueued / ThreadQueueAdmitted / ThreadQueueDropped / \
                 ThreadQueueCompleted stream the queue emits as it decides",
        },
    },
    TableRule {
        table: "thread_summaries",
        owners: &[
            "api/threads_compose.rs",
            "core/image_migration.rs",
            "core/store/threads/backfill.rs",
            "engine/agent_recovery/has_diff.rs",
            "engine/event_bus/mod.rs",
            "engine/event_bus/parent_callback.rs",
            "engine/event_bus_projection_propagation.rs",
            "engine/event_bus_projection_thread.rs",
            "engine/session_seed.rs",
            "main.rs",
        ],
        announcement: Announcement::Projection {
            of: "the events table. Every column is derived from a ThreadEvent \
                 that was already broadcast, and the reconcile/backfill writers \
                 repair drift in that derivation rather than introducing new \
                 state.",
        },
    },
    TableRule {
        table: "webhook_deliveries",
        owners: &["core/webhook_deliveries.rs"],
        announcement: Announcement::Silent {
            reason: "The nonce ledger a delivery claims before it emits. Holds \
                     no payload, is listed nowhere, and its only reader is the \
                     next delivery. What a delivery DID is the pinned domain \
                     event it emitted, which announces already; a second event \
                     per arrival would describe the mechanism rather than a \
                     state change anyone made.",
        },
    },
    TableRule {
        table: "webhooks",
        owners: &["core/webhooks.rs"],
        announcement: Announcement::Announced {
            events: &["WebhookCreated", "WebhookUpdated", "WebhookDeleted"],
            exempt: &[
                ExemptWriter {
                    function: "record_accepted",
                    why: "Stamps when a delivery last verified, which is an \
                          observation rather than a decision. The delivery \
                          already emitted the hook's own pinned domain event, \
                          and WebhookUpdated would put an edit nobody made on \
                          the timeline.",
                },
                ExemptWriter {
                    function: "record_refused",
                    why: "Stamps when a delivery was last turned away. Same \
                          shape as record_accepted, and the refusal reaches \
                          the owner through the row rather than through an \
                          event, since a public endpoint receives thousands of \
                          unsigned probes and each one would be a timeline row.",
                },
            ],
        },
    },
];

/// Every module that mutates the workspace `data/` tree. Unlike a table there is
/// nothing to key the scan on, so the unit is the owning module: a reachable
/// function in one of these that touches the filesystem must announce.
pub const DATA_WRITERS: &[DataWriterRule] = &[
    DataWriterRule {
        owner: "core/artifacts.rs",
        writes: "the data/ git store (artifacts, config, knowhow, plugin files)",
        announcement: Announcement::Announced {
            events: &["ArtifactCreated", "ArtifactUpdated", "ArtifactDeleted"],
            exempt: &[
                ExemptWriter {
                    function: "new",
                    why: "Creates the store's own root directory and the \
                          workspace .gitignore at boot. Infrastructure, not a \
                          user-visible artifact.",
                },
                ExemptWriter {
                    function: "write_artifact",
                    why: "The private raw writer. Unreachable from outside the \
                          module, which is exactly the property that forces \
                          callers through write_and_commit.",
                },
                ExemptWriter {
                    function: "write_batch_and_commit",
                    why: "A bulk import (git clone) lands hundreds of files as \
                          ONE user action, and its caller announces one \
                          RepositoryImported for the batch. An entity event per \
                          file would flood the timeline and re-index each file \
                          separately. Tracked in docs/temporary-measures.md: \
                          the batch should take a WriteAnnouncement too.",
                },
                ExemptWriter {
                    function: "resolve_collision_free_path",
                    why: "Probes for a free filename; the write that follows is \
                          the caller's write_and_commit, which announces.",
                },
            ],
        },
    },
    DataWriterRule {
        owner: "core/apps.rs",
        writes: "data/apps/<id>/",
        announcement: Announcement::Announced {
            events: &["AppCreated", "AppUpdated", "AppDeleted"],
            exempt: &[
                ExemptWriter {
                    function: "new",
                    why: "Creates the apps root directory at boot. \
                          Infrastructure, not an app.",
                },
                ExemptWriter {
                    function: "delete_file_and_commit",
                    why: "Removing one file from an app is not an app lifecycle \
                          change. The caller (engine/tools/files.rs) decides \
                          whether the deletion killed the app, by checking \
                          whether it took manifest.json with it.",
                },
            ],
        },
    },
];

#[cfg(test)]
#[path = "announced_surfaces_tests.rs"]
mod tests;
