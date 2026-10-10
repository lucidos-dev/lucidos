// API response types that match the Rust backend

import type { ActorMode } from '../store/thread-events';
import type { CredentialInfo, CredentialRequest } from '../store/types';
import type { CodingAgent } from '../generated/thread-event-wire';

/** Coding-agent backend discriminator, generated from the Rust `CodingAgent`
 *  enum. Re-exported here because this is where API callers reach for it. */
export type { CodingAgent } from '../generated/thread-event-wire';

export interface ChatRequestBody {
  message: string;
  /**
   * Required: semantic mode of the actor authoring this message.
   * - `'human'`: a real person (default for typical chat).
   * - `'agent'`: LLM-driven (e.g. one thread spawning another via `run_thread`).
   * - `'engine'`: engine-internal (recovery / scheduler).
   * `'agent'` and `'engine'` additionally require `parent_thread_id` (and
   * ideally `spawning_event_id`) so a system-spawned thread always records
   * what spawned it.
   */
  mode: ActorMode;
  model?: string;
  device_id?: string;
  reasoning_effort?: string;
  /** The backend to pin for `model`. Absent lets the thread's memory, then
   *  the model's own default, decide. */
  provider?: string;
  app_context?: {
    app_id: string;
  };
  file_context?: {
    path: string;
    lines?: [number, number];
  };
  url_context?: {
    url: string;
    title?: string;
    content: string;
  };
  repo_file_context?: {
    repo_id: string;
    path: string;
    lines?: [number, number];
  };
  /** Sha256 hashes of blobs already uploaded to `POST /threads/:id/blobs`.
   *  The frontend sends images this way only. */
  image_hashes?: string[];
  /** Inline base64 images, for a caller that cannot upload first, such as a
   *  cross-workspace spawn. The engine stores them as it stores an upload
   *  and sniffs the format, so `mime_type` is optional. */
  images?: Array<{ base64: string; mime_type?: string }>;
  /** True when this request spawns / continues a coding-agent thread (any
   *  backend), as opposed to a chat thread answered by the Lucidos Agent.
   *  The `coding_agent` field below picks the backend. The engine still
   *  accepts the legacy `use_claude_code` key via a serde alias. */
  use_coding_agent?: boolean;
  cc_model?: string;
  /** Which coding-agent backend a NEW coding-agent thread runs on. Requires
   *  `use_coding_agent: true`. Ignored on follow-ups — the thread's stored
   *  backend wins (locked at first SessionStarted). */
  coding_agent?: CodingAgent;
  event_id?: string;
  thread_id?: string;
  /** Set on a send whose `thread_id` names a thread the server has not seen
   *  yet, because THIS request is the one creating it (a raw new send, where
   *  the client mints the uuid). Without it the engine 404s an unknown id
   *  rather than materializing the thread from it, so a caller that reached the
   *  wrong engine finds out instead of getting a phantom thread there.
   *  Follow-ups and compose first-sends omit it: a compose thread already has
   *  its row (`sendCompose` awaits `POST /threads` before the chat POST). */
  new_thread?: boolean;
  /** Required when `mode !== 'human'`: the thread that spawned this one. */
  parent_thread_id?: string;
  /** Required when `mode !== 'human'`: the parent event that triggered the spawn. */
  spawning_event_id?: string;
  conflict_change_id?: string;
  /** Legacy CC scope binding — a registered repo UUID or name. Kept for
   *  back-compat with older callers (CLI, agent_thread spawns); the
   *  compose-view picker now sends `folder` instead, and the engine accepts
   *  either (mutually exclusive: 400 if both are set). */
  repo_id?: string;
  /** New CC scope payload — an absolute path, a workspace-relative path
   *  (`data/apps/<id>`), or a registered repo name/UUID. The engine resolves
   *  via the `coding_agent_kind` pipeline to one of `lucidos | app |
   *  external` and routes the spawn accordingly. Mutually exclusive with
   *  `repo_id`. */
  folder?: string;
  title?: string;
}

export interface ArtifactsResponse {
  artifacts: string[];
}

export interface NotificationsResponse {
  notifications: Array<{
    id: string;
    task_id?: string;
    app_id?: string;
    thread_id?: string;
    title: string;
    message: string;
    created_at: string;
    read: boolean;
  }>;
  unread_count: number;
  has_more: boolean;
}

export interface CredentialsListResponse {
  credentials: CredentialInfo[];
}

/** A user-managed environment variable (Settings → Environment Variables).
 *  Mirrors the engine row exposed by GET /api/v1/env-vars. */
export interface EnvironmentVariable {
  id: string;
  name: string;
  value: string;
  created_at: string;
  updated_at: string;
}

export interface EnvVarsListResponse {
  env_vars: EnvironmentVariable[];
}

/** GET /api/v1/network-config — the per-workspace engine bind plus the inherited
 *  machine-global gateway bind, for Settings → Access → Network access. Mirrors
 *  the engine `NetworkConfigResponse`. `engine_bind` is `loopback` | `all` | an
 *  IP; when `inherit` is true the engine binds `gateway_bind` and the pane
 *  disables the engine field (showing the inherited value). */
export interface NetworkConfigResponse {
  engine_bind: string;
  inherit: boolean;
  gateway_bind: string;
  detected_tailscale_ip: string | null;
}

/** GET /api/v1/tailnet-status, mirroring the engine `TailnetStatusResponse`.
 *  What this machine's tailnet looks like, over plain HTTP, so a phone browser
 *  gets the same answer the packaged desktop app does.
 *
 *  `magic_dns_name` carries no scheme (`<machine>.<tailnet>.ts.net`), and is
 *  null off a tailnet or with MagicDNS turned off.
 *
 *  `workspace_serve_url` is the full `https://<name>/<slug>/` URL, and the
 *  engine sets it ONLY after a request to it came back from that same engine.
 *  Print it verbatim: reassembling it here would let the two disagree about a
 *  string the engine verified and this side did not. */
export interface TailnetStatusResponse {
  magic_dns_name: string | null;
  workspace_serve_url: string | null;
}

/** One coding agent's effective CLI binary resolution — mirrors the engine
 *  `runtime::AgentBinaryStatus`. Live detection: `override` = the
 *  `coding_agent_*_path` preference, `detected` = probe-list hit, `path` =
 *  bare PATH lookup, `not-found` = nothing resolves. `valid` is false when a
 *  set override doesn't point at an executable (a spawn would fail with
 *  `error`). `version` is the binary's own `--version`, parsed to the bare
 *  token (`2.1.224`); absent when nothing resolves or the probe found no
 *  recognizable version, so it is never a placeholder. */
export interface AgentBinaryStatus {
  path: string | null;
  source: 'override' | 'detected' | 'path' | 'not-found';
  valid: boolean;
  error?: string;
  version?: string;
}

/** GET /api/v1/coding-agents/binaries — per-agent binary resolution for
 *  Settings → Coding Agents. Mirrors the engine
 *  `AgentBinariesResponse`. */
export interface AgentBinariesResponse {
  claude_code: AgentBinaryStatus;
  codex: AgentBinaryStatus;
}

/** One way a model can be served: a backend, the id sent to it, and what that
 *  backend offers. Mirrors the engine `api::RouteInfo`.
 *
 *  A row carries an ordered list, which is what lets one model be served by
 *  whichever provider the workspace has credentials for. */
export interface RouteInfo {
  /** 'vertex' | 'anthropic' | 'openai' | 'openrouter' | 'xai' |
   *  'opencode-free' | 'local'. */
  provider: string;
  /** The id this route puts on the wire. The engine always spells it out, even
   *  where the stored route leaves it to default to the row's own id. */
  id: string;
  /** Context window in tokens, or absent when the engine infers it from this
   *  route's id. The id-shape fallback only knows Claude and GPT-5, so every
   *  OpenRouter / xAI / Gemini / local route is treated as 200k until set. */
  context_window?: number;
  /** Reasoning efforts this BACKEND offers for this model, derived by the
   *  engine (`llm::reasoning::supported_efforts`) and the SAME set
   *  `RoutingProvider` clamps a request onto. Per route, because the answer
   *  depends on the backend: a Claude id offers six on Vertex and four through
   *  OpenRouter, whose server has no `xhigh`. */
  reasoning_efforts: string[];
}

/** A chat model in the DB-backed registry (Settings → Models). Mirrors the
 *  engine `api::ModelInfo`. `source` is 'builtin' (disable-only) or 'user'
 *  (deletable). */
export interface ModelInfo {
  id: string;
  label: string;
  /** Backends that can serve this model, in priority order. Never empty. */
  routes: RouteInfo[];
  /** The provider last picked for this model, or null for never picked.
   *  Honoured when its route is configured, and REFUSED rather than
   *  substituted when it is not. */
  preferred_provider: string | null;
  /** The *vision flag*: whether this model reads images. Image description
   *  offers and runs only such models. */
  vision: boolean;
  /** The *default effort*: the tier this model runs at when nothing stored
   *  names one. `null` means the engine sends no effort. */
  default_effort: string | null;
  sort_order: number;
  source: string;
  enabled: boolean;
  created_at: string;
}

export interface ModelsListResponse {
  models: ModelInfo[];
}

/** One row of the *style library*, merged by the engine from what it ships and
 *  what the user saved. `source` is what decides between Reset and Delete. */
export interface ResponseStyle {
  id: string;
  label: string;
  /** The one line the picker shows. Shipped styles carry their own; a user's
   *  own derives it from the first line of the instruction. */
  description: string;
  /** What gets injected, verbatim, under the engine's own heading. Empty for
   *  `standard`, the off switch. */
  instruction: string;
  /** `builtin` ships untouched, `overridden` ships with the user's text on
   *  top, `user` is theirs alone. */
  source: 'builtin' | 'overridden' | 'user';
  /** False for `standard` alone. Sent by the engine rather than derived here,
   *  so the client holds no second copy of the rule. */
  editable: boolean;
}

export interface ResponseStylesListResponse {
  styles: ResponseStyle[];
}

/** The engine's read-back on a trigger's cron after a create or update. Present
 *  only on the trigger write endpoints. */
export interface CronPreview {
  /** The next few upcoming fire times (RFC3339), merged across the whole
   *  expression array. */
  next_runs: string[];
  /** Non-fatal warnings, e.g. the day-of-month/day-of-week AND footgun. A cron
   *  that can never fire is a hard error instead, so it arrives as `error`. */
  warnings: string[];
}

/** Generic success/error response used by credential, preference, and trigger
 *  endpoints. */
export interface ApiResult {
  success: boolean;
  error?: string;
  /** The whole request, not a narrower copy of it. This value is handed
   *  straight to `openCredentialRequest`, so a copy listing four of its fields
   *  only lies about the rest: the OAuth repair's `existing_credential_id` and
   *  `missing` already travelled here undeclared. */
  credential_request?: CredentialRequest;
  auth_url?: string;
  cron_preview?: CronPreview;
  /** Non-fatal notes about a write that succeeded. Today: an event type in a
   *  trigger's `on` list that this workspace has never emitted. Separate from
   *  `cron_preview.warnings`, which is about the schedule. */
  warnings?: string[];
}

export interface UploadResponse {
  success: boolean;
  filename?: string;
  error?: string;
}


export interface TriggersListResponse {
  triggers: import('../store/types').TriggerInfo[];
}

/** Response from an *off-schedule run* (`POST /api/v1/triggers/run`).
 *
 *  `success: true` with `status: 'already-running'` means the request was valid
 *  and NOTHING new started: a fire of this trigger was already active or
 *  queued, and cron fires coalesce to at most one pending run per trigger. It
 *  must never be presented as a started run. `success: false` means refused
 *  (paused trigger, or event-only), with the reason in `message`. */
export interface TriggerRunResult {
  success: boolean;
  status?: 'started' | 'queued' | 'already-running';
  message: string;
}

export interface DeviceInfo {
  id: string;
  name: string | null;
  /** The name the device got when it paired, as the gateway forwarded it. */
  pairing_label: string | null;
  user_agent: string | null;
  push_enabled: boolean;
  last_seen_at: string;
  created_at: string;
}

// --- Memory Inspector ---

export interface MemoryEntrySource {
  type: 'event' | 'artifact';
  id?: string;
  path?: string;
  commit?: string;
}

export interface MemoryEntryInfo {
  id: string;
  source: MemoryEntrySource;
  topic: string;
  summary: string;
  importance: number;
  entities: string[];
  src_created_at: string;
  created_at: string;
}

export interface MemoryEntriesResponse {
  entries: MemoryEntryInfo[];
  total: number;
  has_more: boolean;
}

export interface ImportanceDistribution {
  low: number;
  medium: number;
  high: number;
  critical: number;
}

export interface TopicCount {
  topic: string;
  count: number;
}

/** How far the engine's background embedding-model load has got. Mirrors the
 *  Rust `EmbeddingModelLoadState` (`memory/embedder_slot.rs`); `kind` is the
 *  wire discriminator, pinned by `embedding_model_load_state_wire_tags_are_stable`.
 *
 *  Read two ways, deliberately: live from the `EmbeddingModelStatusChanged` SSE
 *  frame, and as a snapshot from `/memory/embedding-model-status` for a client
 *  that loads mid-download (the normal case on a fresh workspace, where the
 *  download starts at engine boot). Both carry this exact shape. */
export type EmbeddingModelLoadState =
  /** Cache is cold: bytes are being fetched. `total_bytes` is what is known so
   *  far, and the pair is monotonic, so it is safe to render as a bar. */
  | { kind: 'downloading'; downloaded_bytes: number; total_bytes: number }
  /** Files are local; the ONNX session is being built. Seconds, not minutes. */
  | { kind: 'loading' }
  | { kind: 'ready' }
  /** A fetch failed (offline, hub blocked); the loader is backing off. */
  | { kind: 'waiting'; attempt: number }
  /** Terminal: the loader has stopped and only a fix plus a restart changes it. */
  | { kind: 'failed'; message: string };

export interface EmbeddingModelStatus {
  model_id: string;
  load_state: EmbeddingModelLoadState;
}

/** How far the Tree memory module's backfill has got, in trees: each
 *  in-scope thread plus the workspace. Mirrors the Rust `BackfillProgress`
 *  (`summary_tree/module.rs`). The `TreeBackfillProgressed` frame carries the
 *  same object, pinned by `the_progress_frame_carries_the_rest_shape`. */
export interface BackfillProgress {
  done: number;
  /** Zero until the compactor has seeded. */
  total: number;
  /** `done` in thousandths of a tree, plus the built share of each tree under
   *  way. The bar reads it, so it moves within a long tree. Only new work in
   *  a tree under way lowers it. */
  done_milli: number;
  /** Summaries built so far in the trees under way. */
  nodes_done: number;
  /** Summaries those trees need in all. */
  nodes_total: number;
  /** No background model could be resolved, so the backfill cannot move. */
  waiting_for_model: boolean;
  /** A summary failed. Its tree goes back on the queue shortly, and this
   *  holds until that tree completes. */
  retrying: boolean;
  /** The ready flag is set: turns read the trees while older threads still
   *  fill in. */
  ready: boolean;
  /** The order of the frame that announced this count. A snapshot carries the
   *  last frame's order, so a frame no newer than it was already counted. */
  seq: number;
}

/** `GET /api/v1/memory/tree-backfill`. Mirrors the Rust `TreeBackfill`. */
export type TreeBackfill =
  /** `started`: a backfill wrote summaries before, so Tree resumes with no
   *  new confirm. */
  | { state: 'off'; started: boolean }
  | { state: 'running'; progress: BackfillProgress }
  /** `filling` counts the older threads while they are still being built. */
  | { state: 'ready'; filling?: BackfillProgress };

/** One summary tree line, as `/api/v1/recall/*` and `/api/v1/memory/tree`
 *  serve it. `id` is `w/start+span` or `<thread id>/start+span`; the line
 *  covers `span` entries. `w/pending` counts entries not summarised yet. */
export interface RecallLine {
  id: string;
  text: string;
}

/** `GET /api/v1/memory/tree`. Mirrors the Rust `TreeTop` (`summary_tree/recall.rs`). */
export interface SummaryTreeTop {
  entries: number;
  /** The largest blocks tiling the tree's entries, oldest first. */
  lines: RecallLine[];
}

/** `GET /api/v1/recall/zoom`. A leaf opens into one line with its own id:
 *  the exact message, or the artifact text. */
export interface RecallZoomResponse {
  id: string;
  lines: RecallLine[];
}

/** `GET /api/v1/recall/date`. RFC 3339 times. */
export interface RecallDateResponse {
  id: string;
  from: string;
  to: string;
}

/** One row of `GET /api/v1/memory/tree/threads`. Mirrors the Rust `TreeThread`. */
export interface SummaryTreeThread {
  thread_id: string;
  title: string | null;
  last_activity: string;
  /** Entries of its tree the compactor has summarised. */
  summarised: number;
}

export interface SummaryTreeThreadsResponse {
  threads: SummaryTreeThread[];
  total: number;
  has_more: boolean;
}

export interface Bounds {
  low: number;
  high: number;
}

/** One reasoning tier's cost and time. `backfill_usd`/`daily_usd` are the
 *  range; `*_central` is the one figure the panel leads with, from measured
 *  (or seeded) per-call tokens rather than the range's own guesses. */
export interface EffortCost {
  effort: string;
  /** Until turns read the trees, at this tier's seconds per call. */
  usable_secs: Bounds;
  /** Until every tree is built, at this tier's seconds per call. */
  complete_secs: Bounds;
  backfill_usd: Bounds;
  backfill_usd_central: number;
  daily_usd: Bounds;
  daily_usd_central: number;
}

/** `GET /api/v1/memory/tree-backfill/estimate`. Mirrors the Rust
 *  `TreeBackfillEstimate` (`summary_tree/estimate.rs`). Every figure is a
 *  range, because entry sizes are approximated. */
export interface TreeBackfillEstimate {
  calls: Bounds;
  /** Until turns read the trees, at the default seconds per call: what a
   *  model the estimate does not price shows. */
  usable_secs: Bounds;
  /** Until every tree is built, at the default seconds per call. */
  complete_secs: Bounds;
  daily_calls: Bounds;
  /** One per compactor model, each carrying a row per reasoning tier. A
   *  tier the model cannot run is priced as the tier routing snaps it to. */
  costs: { model: string; by_effort: EffortCost[] }[];
}

/** One row of `GET /api/v1/models/background`. Mirrors the Rust
 *  `BackgroundModel` (`api/background_models.rs`): the model a background task
 *  runs on, and where it came from. */
export interface BackgroundModel {
  model: string;
  effort: string | null;
  source: 'preference' | 'default' | 'chat-model';
  /** Whether a configured provider serves `model` and has not answered
   *  not-found for it. Only a stored pick or the chat model can be
   *  unreachable, and its calls then fail. */
  reachable: boolean;
  /** The models this row would run on ahead of `model`, plus `model`, whose
   *  provider answered not-found in the last six hours (ADR 0403). */
  not_served: string[];
  /** Whether this task sends images, so its model must read them. */
  needs_vision: boolean;
  /** Whether `model` reads images. With `needs_vision`, `false` means the
   *  engine refuses the task rather than calling the model. */
  vision: boolean;
  /** The models the picker lists first, best first. */
  recommended: RecommendedSelection[];
}

/** One recommended model, and the tier its picker badges as recommended.
 *  Mirrors the Rust `RecommendedSelection`. `effort` is set only where
 *  measurement backs the tier: the compactor's row. */
export interface RecommendedSelection {
  model: string;
  effort: string | null;
}

/** `GET /api/v1/models/background`, keyed by each row's model preference. */
export type BackgroundModels = Record<string, BackgroundModel>;

export interface MemoryStatsResponse {
  total: number;
  event_count: number;
  artifact_count: number;
  importance_distribution: ImportanceDistribution;
  top_topics: TopicCount[];
}

export interface MemorySourceResponse {
  source_type: 'event' | 'artifact';
  event?: {
    id: string;
    event_type: string;
    payload: unknown;
    created: string;
  };
  artifact?: {
    path: string;
    commit: string;
    content: string;
  };
  entries: MemoryEntryInfo[];
}
