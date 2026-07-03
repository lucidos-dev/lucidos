// --- Async data loading ---
// Every piece of async data must be in one of these states.
// Components must handle all four (and both loaded sub-states: empty vs hits).
// Failed MUST render differently from empty — "No results" is not the same as
// "Something went wrong".
export type Loadable<T> =
  | { status: 'not-loaded' }
  | { status: 'loading' }
  | { status: 'loaded'; data: T }
  | { status: 'failed'; error: string; httpCode?: number };

/** Extract data from a Loadable, returning the provided fallback when not loaded. */
export function loadedOr<T>(loadable: Loadable<T>, fallback: T): T {
  return loadable.status === 'loaded' ? loadable.data : fallback;
}

// Convert an unknown error into a Loadable failed state.
// Preserves httpCode from ApiError instances.
export function toFailed<T>(error: unknown): Loadable<T> {
  // Inline check to avoid circular import with api/client.ts
  if (error instanceof Error && 'httpCode' in error && 'reason' in error) {
    return { status: 'failed', error: (error as { reason: string }).reason, httpCode: (error as { httpCode: number }).httpCode };
  }
  return { status: 'failed', error: error instanceof Error ? error.message : String(error) };
}

/** Flip a signal to `loading` only if it isn't already `loaded`. Used by
 *  refetchers (filter change, SSE refresh, post-mutation reload) so the
 *  visible list stays through the network round-trip and swaps atomically
 *  when fresh data lands — without this, every refetch flashes a spinner. */
export function setLoadingIfFresh<T>(signal: { value: Loadable<T> }): void {
  if (signal.value.status !== 'loaded') {
    signal.value = { status: 'loading' };
  }
}

// Menu item names (drawer navigation)
export const MENU_ITEMS = ['files', 'apps', 'plugins', 'triggers', 'settings', 'changes', 'notifications'] as const;
export type MenuItem = typeof MENU_ITEMS[number];

// Connection status. 'connecting' is the initial state before the first
// /health poll resolves — it renders the dot neutral grey (no red, no blink),
// so a page refresh never flashes red while the first check is in flight. The
// 5s poll only ever resolves to 'connected' or 'disconnected' thereafter.
export type ConnectionStatus = 'connected' | 'disconnected' | 'connecting';

// A single step in chat processing (tool call, memory search, etc.)
export interface Step {
  description: string;
  success: boolean | null; // null = pending/in-progress
  /** Pairs the step with its CodingAgentToolResult by id; description is
   *  ambiguous for parallel calls like two `Read SKILL.md`. Absent for
   *  engine tools and legacy DB rows. */
  tool_use_id?: string;
  /** Legacy fields — kept for old DB rows. New emissions carry these
   *  on `contextCapture.usage`. */
  context_tokens?: number;
  context_messages?: number;
  trimmed?: boolean;
  /** Absent for steps that aren't an LLM call (memory search, tool result). */
  contextCapture?: ContextCapture;
  /** Accumulated reasoning/thinking text streamed for this "Thinking" step
   *  (from `CodingAgentThoughtStreamed` / chat `ThoughtStreamed`). Rendered as
   *  the step's expandable live content so a long reasoning pass shows progress
   *  instead of a bare "Thinking" label. Absent on non-thinking steps. */
  thinkingText?: string;
}

/** API role bucket a `ContextSection` belongs to. Mirrors the three buckets
 *  in the LLM API call: the system prompt, prior messages (verbatim resume
 *  tool blocks), and this turn's user message. Wire values come from the
 *  Rust `ContextRole` enum with `#[serde(rename_all = "snake_case")]`. */
export type ContextRole = 'system' | 'prior_message' | 'user';

/** One labeled chunk of the LLM's assembled prompt. `char_count` is the
 *  original length; `content` is omitted when the `capture_context`
 *  preference is off (the modal still renders the section with its name and
 *  size, just without the body). */
export interface ContextSection {
  name: string;
  content?: string;
  char_count: number;
  /** Optional for backward compatibility with snapshots persisted before
   *  role-tagging existed. Default is `'user'` (matches Rust
   *  `default_context_role`). */
  role?: ContextRole;
  /** Inner-group label used by the viewer to nest sections within the
   *  user-message role. Absent for system-role sections, prior-message rows,
   *  and legacy events. */
  group?: string;
}

/** `cache_*` are Anthropic-only; zero on OpenAI / Gemini. */
export interface ApiUsage {
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_creation_tokens: number;
}

export type ContextProducer = 'main_llm' | 'claude_code' | 'codex';

/** Mirrors the Rust `ContextCaptured` ThreadEvent. `usage` is absent on
 *  pre-call snapshots and on providers that don't report it (OpenAI,
 *  Gemini). `legacy` is set by `synthesizeContextCapture` for old rows.
 *
 *  `sections_stripped` + `event_id` cover the lazy-load contract: the
 *  snapshot endpoint drops `sections` and `tools` to keep the events
 *  list cheap (a single capture can be ~50 kB; a heavy thread carries
 *  hundreds). When the user opens the step-detail modal, the frontend
 *  fetches the full sections + tools via `GET /events/:event_id/context`. */
export interface ContextCapture {
  producer: ContextProducer;
  model: string;
  context_window: number;
  sections: ContextSection[];
  tools: string[];
  estimated_total_tokens: number;
  usage?: ApiUsage;
  trimmed: boolean;
  legacy?: boolean;
  /** Server stripped sections + tools on this snapshot. Modal must lazy-fetch. */
  sections_stripped?: boolean;
  /** Event id used by the lazy-fetch endpoint. Stamped by `capturedEventToData`
   *  from the originating row. Absent on legacy syntheses (those have no single
   *  source event to fetch). */
  event_id?: string;
}

/** @deprecated kept for the `ResponseEvent.context` slot until that wiring
 *  is migrated. New code should use `ContextCapture`. */
export interface ContextAssembledData {
  sections: ContextSection[];
  tools: string[];
  model: string;
  total_chars: number;
}

// A response event — interleaved text blocks and steps (from backend ResponseEvent).
export type ResponseEvent =
  | { type: 'text'; md: string }
  | {
      type: 'step';
      description: string;
      tool_name?: string;
      success: boolean | null;
      tool_use_id?: string;
      detail?: string;
      /** Legacy fields — see Step.context_tokens above. */
      context_tokens?: number;
      context_messages?: number;
      trimmed?: boolean;
      full?: string;
      created?: string;
      result?: string;
      result_images?: string[];
      /** `true` when the source ToolResult event had its `result` field
       *  stripped on the snapshot endpoint (see `strip_tool_result_content`
       *  in `api/threads.rs`). Paired with `result_event_id` so the
       *  step-detail modal can lazy-fetch the full text on open. */
      result_stripped?: boolean;
      /** The `_eventId` of the source ToolResult event — populated whenever
       *  the snapshot replay routed the result onto this step. Live SSE
       *  emits also stamp it. Used as the route key for
       *  `GET /events/:event_id/tool-result`. */
      result_event_id?: string;
      /** @deprecated kept for legacy backend payloads. */
      context?: ContextAssembledData;
      contextCapture?: ContextCapture;
      /** Accumulated reasoning text for a "Thinking" step — see Step.thinkingText. */
      thinkingText?: string;
    }
  | { type: 'section_break'; channel: string }
  | { type: 'image'; base64: string; mime_type: string; index: number }
  | {
      type: 'question';
      tool_use_id: string;
      question: string;
      options: Array<{ id: string; label: string; description?: string }>;
      /** When set, the user already picked / typed / canceled; the card renders the resolved state instead of action buttons. */
      resolved?: { kind: 'Selected'; option_id: string } | { kind: 'FreeText'; text: string } | { kind: 'Canceled' };
    }
  | {
      /** CC requested permission for an out-of-cwd / .claude / Bash tool call.
       *  The card resolves when the user clicks Allow or Deny — the resolution
       *  arrives as a paired CodingAgentPermissionResolved event. */
      type: 'permission';
      request_id: string;
      tool_use_id: string;
      tool_name: string;
      input: Record<string, unknown>;
      summary: string;
      resolved?: { allowed: boolean; reason?: string };
    }
  | {
      /** The command guard checkpointed the workspace before a ReversibleDanger
       *  command (ADR 0002, Phase 4). Renders inline with a one-click Undo;
       *  `reverted` flips true once the paired CommandCheckpointReverted event
       *  lands in this exchange. */
      type: 'checkpoint';
      checkpoint_id: string;
      command: string;
      summary: string;
      reverted: boolean;
    }
  | {
      /** The model ended its turn cleanly but produced no text (a benign empty
       *  completion — emitted as an empty `ResponseGenerated`). Rendered as a
       *  neutral note so the exchange clearly states what happened instead of
       *  showing a blank body or a red error. Synthesised by
       *  `exchangeResponseEvents`, never sent by the backend. */
      type: 'empty';
    };

import type { Tap } from '@lucidos/sdk';

// A notification
export interface Notification {
  id: string;
  task_id?: string;
  app_id?: string;
  /** Originating thread, when the notification has one. Used by the §4
   *  in-app matrix to drive Row 1's auto-mark-read when the user is
   *  looking at the source thread. Distinct from `tap.to.id` on a
   *  `tap.kind === 'navigate'` to a thread — that's the navigation target. */
  thread_id?: string;
  /** Specific event UUID inside `thread_id` that raised this notification —
   *  used by the §4 in-app matrix to silently mark-read when the user is
   *  looking at the source event. Distinct from `tap.to.event_id` (which
   *  controls the scroll-and-pulse target when the tap navigates). */
  event_id?: string;
  title: string;
  message: string;
  created_at: string;
  read: boolean;
  tap?: Tap;
}

export type TriggerRun =
  | { type: 'intent'; intent: string }
  | { type: 'script'; path: string };

/** An irreversible-side-effect category a trigger can be granted (ADR 0002,
 *  Phase 5). Wire form is snake_case, matching the Rust `SideEffectCategory`
 *  enum. Used by the *command guard* to decide whether an unattended trigger
 *  may run an `IrreversibleDanger` command. Keep in sync with
 *  `SIDE_EFFECT_CATEGORIES` (the labelled UI list) and the Rust enum. */
export type SideEffectCategory =
  | 'email'
  | 'external_api'
  | 'cloud_cli'
  | 'out_of_workspace_destruction'
  | 'other';

/** One event a trigger listens for, with an optional payload filter scoped
 *  to that event. A trigger fires when an incoming event matches any entry's
 *  `event_type` AND that entry's `condition` (if set) evaluates true against
 *  the payload. Conditions are per-entry, so a single trigger can subscribe
 *  to events with different payload shapes without one filter constraining
 *  the others. */
export interface EventSubscription {
  event_type: string;
  condition?: Record<string, unknown>;
}

// A trigger config (event-sourced).
// Configs may be schedule-only, event-only, or hybrid; `deriveTriggerType()`
// classifies the config shape. The actual invocation that fired a given run
// is recorded on `TriggerStarted.invocation`, not derived from the config.
export interface TriggerInfo {
  id: string;
  name: string;
  cron_expressions: string[];
  timezone: string;
  paused: boolean;
  last_run?: string;
  /** Outcome of the most recent completed firing. Absent until the trigger has
   *  run at least once under an engine that records status (legacy runs show the
   *  `last_run` timestamp only). Surfaced as an OK/failed chip on the row. */
  last_run_status?: 'ok' | 'failed';
  next_run?: string;
  run: TriggerRun;
  /** Event subscriptions. Engine omits the field when there are none, so
   *  readers must tolerate absence. */
  on?: EventSubscription[];
  app_id?: string;
  /** When true, threads spawned by this trigger surface in REVIEW on completion
   *  instead of going straight to ARCHIVE. Absent or false = ARCHIVE (default). */
  go_to_review?: boolean;
  /** Owning *trigger group* id; absent renders the trigger under the implicit
   *  "Ungrouped" section in the panel. Orthogonal to `app_id`. */
  group_id?: string;
  /** The trigger's *side-effect grant* (ADR 0002, Phase 5) — the irreversible
   *  side-effect categories it may perform unattended. Engine omits the field
   *  when empty, so readers must tolerate absence (= no grant). */
  side_effect_grant?: SideEffectCategory[];
  /** Plugin provenance (ADR 0019) — the id of the *plugin* that auto-registered
   *  this trigger. Absent for user-created triggers. Drives the "from <plugin>"
   *  chip in the triggers panel. */
  plugin_id?: string;
}

/** A user-visible folder that organizes triggers in the panel. Pure label —
 *  belongs to no agent, has no schedule, runs no code. Sorted by `order` asc;
 *  ties broken by `created`. */
export interface TriggerGroup {
  id: string;
  name: string;
  order: number;
  /** RFC3339 UTC timestamp from the originating `TriggerGroupCreated` event. */
  created: string;
  /** Number of triggers whose `group_id` references this group at fetch time.
   *  Used by the panel to render the "(N)" badge on the section header. */
  member_count: number;
}

// --- Thread Queue (admission control for the shared thread pool) ---

/** One occupant of the shared pool — waiting for capacity (`queued`) or
 *  actively running (`admitted`). Background kinds come from the
 *  `thread_queue` projection; `user-chat` is user-initiated work merged from
 *  the engine's in-memory state (it shares the pool but isn't persisted).
 *  Mirrors the Rust `ThreadQueueEntry` wire struct. */
export interface ThreadQueueEntry {
  id: string;
  kind: 'event-trigger' | 'cron' | 'sub-thread' | 'coding-agent' | 'user-chat';
  trigger_id?: string;
  trigger_name?: string;
  thread_id?: string;
  summary: string;
  status: 'queued' | 'admitted';
  queued_at: string;
  admitted_at?: string;
}

/** The *capacity policy* governing the Thread Queue. Mirrors the Rust
 *  `CapacityPolicy`. Concurrency caps of 0 mean "hold" (pause admission). */
export interface CapacityPolicy {
  max_concurrent_total: number;
  max_concurrent_event_trigger: number;
  max_concurrent_cron: number;
  max_concurrent_sub_thread: number;
  max_concurrent_coding_agent: number;
  max_concurrent_per_trigger: number;
  max_queued_per_trigger: number;
  /** Slots background work can always reclaim ahead of user-initiated work,
   *  so user priority can't starve triggers/cron. */
  reserved_background: number;
  overflow: 'drop-oldest' | 'pause-trigger';
}

/** GET /api/v1/thread-queue response. */
export interface ThreadQueueResponse {
  entries: ThreadQueueEntry[];
  policy: CapacityPolicy;
}

/** An active (non-paused) trigger has no more runs when it has no next_run and no event subscriptions.
 *  Paused triggers are "Paused", not "No more runs". */
export function hasNoMoreRuns(trigger: TriggerInfo): boolean {
  return !trigger.paused && !trigger.next_run && !(trigger.on && trigger.on.length > 0);
}

/** A trigger that has ever spawned a thread. `name` and `last_activity` are
 *  taken from the most-recent thread; `name` is null when no thread captured
 *  one. `last_activity` lets the UI disambiguate same-named entries. */
export interface HistoricalTriggerInfo {
  id: string;
  name: string | null;
  last_activity: string;
}

export type TriggerType = 'schedule' | 'event' | 'hybrid';

export function deriveTriggerType(trigger: TriggerInfo): TriggerType {
  const hasCron = trigger.cron_expressions.length > 0;
  const hasEvent = !!(trigger.on && trigger.on.length > 0);
  if (hasCron && hasEvent) return 'hybrid';
  if (hasEvent) return 'event';
  return 'schedule';
}

export type AuthType = 'api_key' | 'bearer' | 'basic' | 'password' | 'oauth_client' | 'email_password';

// A credential
export interface CredentialInfo {
  service_name: string;
  base_url: string;
  auth_type: AuthType;
  auth_header: string;
  created_at: string;
  /** Custom env var name the credential's secret is exposed under, overriding
   *  the default `CRED_<NAME>`. `null` when using the default. */
  env_var_name?: string | null;
}

// Email account server settings (no password) — used to pre-fill the edit form
// for an `email_password` credential. Mirrors the engine's `EmailAccountInfo`.
export interface EmailAccountInfo {
  id: string;
  name: string;
  email_address: string;
  imap_host: string;
  imap_port: number;
  smtp_host: string;
  smtp_port: number;
  username: string;
  use_tls: boolean;
  require_send_confirmation: boolean;
  oauth_account_id: string | null;
  created_at: string;
  updated_at: string;
}

// An OAuth connected account
export interface OAuthAccountInfo {
  id: string;
  provider: string;
  email: string | null;
  display_name: string | null;
  scopes: string;
  created_at: string;
  updated_at: string;
}

// An app definition — the app IS the UI component (flat structure)
export interface App {
  id: string;
  name: string;
  description: string;
  icon?: string;
}

// A pinned app entry
export interface PinnedAppEntry {
  app_id: string;
}

// An installed plugin, from GET /plugins/installed (the PluginInstalled event
// projection — no marketplace scan). The Plugins → Installed tab lists these
// regardless of content; `content`/`files` drive the per-plugin navigation
// (open the app, preview a shipped knowhow/script file).
export interface InstalledPlugin {
  id: string;
  name: string;
  version: string;
  source?: string;
  app_id?: string;
  content: string[];
  files: string[];
  /** True when the user has locally modified the plugin's shipped content since
   *  install (derived server-side by diffing the current content against the
   *  install commit). Drives the Plugins-list "Modified" badge. */
  modified?: boolean;
  /** The data/-relative paths that currently differ from the install (the badge
   *  tooltip). Empty/absent unless `modified`. */
  modified_paths?: string[];
}


// Confirm dialog state
export interface ConfirmState {
  visible: boolean;
  message: string;
  okLabel: string;
  title?: string;
  cancelLabel?: string;
  variant?: 'danger' | 'default';
  resolve?: (value: boolean) => void;
  extraAction?: ToastAction;
  details?: ConfirmDetails;
}

export interface ConfirmDetails {
  intro?: string;
  groups: ConfirmDetailGroup[];
}

export interface ConfirmDetailGroup {
  header: string;
  items: string[];
}

// Prompt dialog state — a text-input sibling of ConfirmState. Resolves the
// entered string on OK, or null on Cancel / Esc / backdrop.
export interface PromptState {
  visible: boolean;
  message: string;
  title?: string;
  defaultValue?: string;
  placeholder?: string;
  okLabel?: string;
  cancelLabel?: string;
  multiline?: boolean;
  resolve?: (value: string | null) => void;
}

// Toast notification
export type ToastType = 'success' | 'info' | 'error' | 'warning';

export interface ToastAction {
  label: string;
  onClick: () => void;
  /** Visual intent. Default = neutral; 'danger' = destructive; 'confirm' = positive. */
  variant?: 'danger' | 'confirm';
}

export interface ToastItem {
  id: number;
  message: string;
  type: ToastType;
  key?: string;
  action?: ToastAction;
  /** Rendered next to `action`. The restart-required toast uses this for
   *  "Dismiss" so the explicit affordance sits beside "Restart" instead of
   *  hiding behind the close (X) button. */
  secondaryAction?: ToastAction;
  onClick?: () => void;
  spinning?: boolean;
  /** false = suppress the close (X) button. Used for "Restarting engine…" so
   *  the user can't dismiss the toast while the dim restart overlay is still
   *  blocking the UI behind it. Defaults to true (close button shown). */
  dismissable?: boolean;
  /** true = do NOT auto-focus this toast's button when it appears. Set for
   *  UNSOLICITED toasts (notification toasts) that pop without a user action —
   *  stealing keyboard focus mid-typing (and pre-arming a reflexive Enter on
   *  "OK") would be hostile. The buttons stay Tab-reachable. Solicited action
   *  toasts (e.g. Apply-All) leave it unset so Enter acts immediately. */
  noAutofocus?: boolean;
  /** Desktop pane this toast is centered over, FROZEN to whichever pane was
   *  focused when the toast first appeared (drawer counts as 'thread'). It never
   *  changes afterwards — a later focus switch must not make the toast jump
   *  panes. Set once in `showToast`; a keyed in-place update keeps the original.
   *  Drives `data-toast-pane` in Toast.tsx (desktop CSS only; ignored on mobile,
   *  where one pane fills the screen). */
  pane?: 'thread' | 'content';
}

// Credential request from SSE (engine needs credentials)
export interface CredentialRequest {
  service?: string;
  base_url?: string;
  auth_type?: AuthType;
  prompt?: string;
  /** Pre-fill values for `oauth_client` requests. The agent supplies these from
   *  the `oauth-providers` system-knowhow (auth/token/userinfo URLs + default
   *  scopes) so the modal pre-fills them. Absence — or a missing field —
   *  signals "not pre-filled": the modal auto-expands its endpoint section so
   *  the user fills the URLs in by hand. */
  defaults?: {
    auth_url?: string;
    token_url?: string;
    userinfo_url?: string | null;
    scopes?: string;
  };
  /** Optional custom env var name the agent passed via `request_credential`.
   *  Pre-fills the modal's "Env var name" field so the secret also injects under
   *  this exact name (in addition to the default `CRED_<NAME>`). The user can
   *  edit or clear it before saving. */
  env_var_name?: string;
}

export interface PluginMarketplace {
  id: string;
  name: string;
  source: string;
}

export type MarketplacePluginStatus = 'available' | 'installed' | 'update_available';

export interface MarketplacePlugin {
  marketplace_id: string;
  marketplace_name: string;
  id: string;
  name: string;
  description: string;
  version: string;
  source: string;
  manifest: Record<string, unknown>;
  content: string[];
  /** Topical categories (controlled vocabulary) the plugin is tagged with.
   *  Drives the Store tab's category filter/chips. Unknown author tags are
   *  filtered out engine-side, so every value here is browsable. */
  categories: string[];
  files_count: number;
  status: MarketplacePluginStatus;
  installed_version?: string;
  /** Setup thread spawned at install (installed plugins that shipped a `setup`
   *  field only). The card's "Setup" button opens it. */
  setup_thread_id?: string;
  /** True once the setup thread has finished — flips the card button from
   *  "Setup" to "Open". Absent/false for plugins without setup. */
  setup_complete?: boolean;
  /** The plugin's primary app (`data/apps/<id>/`), if it ships one. The card's
   *  "Open" button launches it; absent → nothing to open. */
  app_id?: string;
  /** True when the user has locally modified the plugin's shipped content since
   *  install (overlaid from the installed projection). Always false/absent for an
   *  available (not-installed) catalog row. Drives the "Modified" badge and the
   *  update-overwrite warning. */
  modified?: boolean;
  /** The data/-relative paths that currently differ from the install (the badge
   *  tooltip + the update-warning detail). Empty/absent unless `modified`. */
  modified_paths?: string[];
}

export interface MarketplaceScanError {
  marketplace_id: string;
  marketplace_name: string;
  source: string;
  error: string;
}

export interface MarketplaceCatalog {
  marketplaces: PluginMarketplace[];
  plugins: MarketplacePlugin[];
  errors: MarketplaceScanError[];
}

/** Plugin install awaiting user confirmation in the install panel. Mirrors
 *  the JSON payload built by `engine::tools::plugins::prepare_install_request`.
 *  The user clicks Confirm/Cancel; the panel POSTs to
 *  `/api/v1/plugins/install/:install_id/{confirm|cancel}` to resolve. */
export interface PluginInstallRequest {
  install_id: string;
  source: string;
  source_type: 'git' | 'archive';
  /** Raw manifest as parsed from `manifest.toml` — id/version/name/description
   *  required, source/engine/setup optional, plus any extra forward-compatible
   *  fields. Render the four required ones; show the rest only on demand. */
  manifest: Record<string, unknown>;
  /** `data/`-relative paths the install will write. */
  files: string[];
  /** Subset of `files` that already exist under `data/` and would be
   *  overwritten. Highlight separately from the rest. */
  overwrites: string[];
  /** Optional post-install instruction text from the manifest. Render as
   *  markdown so the LLM-shippable instructions wrap nicely. */
  setup?: string | null;
  plugin_id: string;
  plugin_version: string;
  plugin_name: string;
}

/** Plugin uninstall awaiting user confirmation in the uninstall panel.
 *  Mirrors the JSON payload built by
 *  `engine::tools::plugins::prepare_uninstall_request`. The user clicks
 *  Confirm/Cancel; the panel POSTs to
 *  `/api/v1/plugins/uninstall/:uninstall_id/{confirm|cancel}` to resolve. */
export interface PluginUninstallRequest {
  uninstall_id: string;
  plugin_id: string;
  plugin_version: string;
  plugin_name: string;
  /** Recorded files that exist on disk RIGHT NOW (will be deleted on Confirm). */
  files_present: string[];
  /** Recorded files that were ALREADY gone at prepare time (manually
   *  deleted, or shared with another plugin). Shown for transparency
   *  but never re-deleted. */
  files_missing: string[];
}

// Email confirmation request from SSE (engine wants user to confirm sending)
export interface EmailConfirmRequest {
  to: string[];
  subject: string;
  body: string;
  cc?: string[];
  bcc?: string[];
  reply_to_message_id?: string;
  account: string;
  from: string;
  attachments?: string[];
  attachment_names?: string[];
}
