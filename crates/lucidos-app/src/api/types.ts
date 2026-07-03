// API response types that match the Rust backend

import type { ActorMode } from '../store/thread-events';
import type { AuthType, CredentialInfo } from '../store/types';

/** Coding-agent backend discriminator — mirrors the Rust `CodingAgent` enum
 *  (kebab-case wire values). */
export type CodingAgent = 'claude-code' | 'codex';

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
  app_context?: {
    app_id: string;
  };
  file_context?: {
    path: string;
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
   *  The frontend's primary path; `images` (legacy base64) is kept on the
   *  body schema for backward compatibility with older callers but the new
   *  path uses `image_hashes` exclusively. */
  image_hashes?: string[];
  /** Legacy: inline base64 images. Kept for compat during the Phase 5
   *  cleanup window; new code uses `image_hashes`. */
  images?: Array<{ base64: string; mime_type: string }>;
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
 *  machine-global gateway bind, for Settings → System → Network access. Mirrors
 *  the engine `NetworkConfigResponse`. `engine_bind` is `loopback` | `all` | an
 *  IP; when `inherit` is true the engine binds `gateway_bind` and the pane
 *  disables the engine field (showing the inherited value). */
export interface NetworkConfigResponse {
  engine_bind: string;
  inherit: boolean;
  gateway_bind: string;
  detected_tailscale_ip: string | null;
}

/** One coding agent's effective CLI binary resolution — mirrors the engine
 *  `runtime::AgentBinaryStatus`. Live detection: `override` = the
 *  `coding_agent_*_path` preference, `detected` = probe-list hit, `path` =
 *  bare PATH lookup, `not-found` = nothing resolves. `valid` is false when a
 *  set override doesn't point at an executable (a spawn would fail with
 *  `error`). */
export interface AgentBinaryStatus {
  path: string | null;
  source: 'override' | 'detected' | 'path' | 'not-found';
  valid: boolean;
  error?: string;
}

/** GET /api/v1/coding-agents/binaries — per-agent binary resolution for
 *  Settings → System → Coding agents. Mirrors the engine
 *  `AgentBinariesResponse`. */
export interface AgentBinariesResponse {
  claude_code: AgentBinaryStatus;
  codex: AgentBinaryStatus;
}

/** A chat model in the DB-backed registry (Settings → Models). Mirrors the
 *  engine `core::models::Model`. `provider` is the backend that serves it
 *  ('vertex' | 'anthropic' | 'openai' | 'openrouter' | 'local'); `source` is
 *  'builtin' (disable-only) or 'user' (deletable). */
export interface ModelInfo {
  id: string;
  label: string;
  provider: string;
  sort_order: number;
  source: string;
  enabled: boolean;
  created_at: string;
}

export interface ModelsListResponse {
  models: ModelInfo[];
}

/** Generic success/error response used by credential, preference, and trigger endpoints. */
export interface ApiResult {
  success: boolean;
  error?: string;
  credential_request?: { service: string; prompt: string; base_url: string; auth_type: AuthType };
  auth_url?: string;
}

export interface UploadResponse {
  success: boolean;
  filename?: string;
  error?: string;
}


export interface TriggersListResponse {
  triggers: import('../store/types').TriggerInfo[];
}

export interface DeviceInfo {
  id: string;
  name: string | null;
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
