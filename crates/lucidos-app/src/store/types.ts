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
export const MENU_ITEMS = ['files', 'apps', 'triggers', 'settings', 'changes', 'notifications'] as const;
export type MenuItem = typeof MENU_ITEMS[number];

// Connection status
export type ConnectionStatus = 'connected' | 'disconnected';

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
}

/** One labeled chunk of the LLM's assembled prompt. `char_count` is the
 *  original length; `content` is omitted when the `capture_context`
 *  preference is off (the modal still renders the section with its name and
 *  size, just without the body). */
export interface ContextSection {
  name: string;
  content?: string;
  char_count: number;
}

/** `cache_*` are Anthropic-only; zero on OpenAI / Gemini. */
export interface ApiUsage {
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_creation_tokens: number;
}

export type ContextProducer = 'main_llm' | 'claude_code';

/** Mirrors the Rust `ContextCaptured` ThreadEvent. `usage` is absent on
 *  pre-call snapshots and on providers that don't report it (OpenAI,
 *  Gemini). `legacy` is set by `synthesizeContextCapture` for old rows. */
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
      /** @deprecated kept for legacy backend payloads. */
      context?: ContextAssembledData;
      contextCapture?: ContextCapture;
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
    };

// A notification
export interface Notification {
  id: string;
  task_id?: string;
  app_id?: string;
  /** Originating thread, when the notification has one. Drives the inbox
   *  modal's "Open thread" action. */
  thread_id?: string;
  title: string;
  message: string;
  created_at: string;
  read: boolean;
}

export type TriggerRun =
  | { type: 'intent'; intent: string }
  | { type: 'script'; path: string };

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
  next_run?: string;
  run: TriggerRun;
  on?: string;
  condition?: Record<string, unknown>;
  app_id?: string;
  /** When true, threads spawned by this trigger surface in REVIEW on completion
   *  instead of going straight to HISTORY. Absent or false = HISTORY (default). */
  go_to_review?: boolean;
}

/** An active (non-paused) trigger has no more runs when it has no next_run and no event trigger.
 *  Paused triggers are "Paused", not "No more runs". */
export function hasNoMoreRuns(trigger: TriggerInfo): boolean {
  return !trigger.paused && !trigger.next_run && !trigger.on;
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
  const hasEvent = !!trigger.on;
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
  created_at: string;
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
  knowhow: string[];
}

// A pinned app entry
export interface PinnedAppEntry {
  app_id: string;
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
}

// Credential request from SSE (engine needs credentials)
export interface CredentialRequest {
  service?: string;
  base_url?: string;
  auth_type?: AuthType;
  prompt?: string;
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
