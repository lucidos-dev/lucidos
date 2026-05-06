// API response types that match the Rust backend

import type { ActorMode } from '../store/thread-events';

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
  images?: Array<{ base64: string; mime_type: string }>;
  use_claude_code?: boolean;
  cc_model?: string;
  event_id?: string;
  thread_id?: string;
  /** Required when `mode !== 'human'`: the thread that spawned this one. */
  parent_thread_id?: string;
  /** Required when `mode !== 'human'`: the parent event that triggered the spawn. */
  spawning_event_id?: string;
  conflict_change_id?: string;
  repo_id?: string;
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
    title: string;
    message: string;
    created_at: string;
    read: boolean;
  }>;
  unread_count: number;
  has_more: boolean;
}

import type { AuthType } from '../store/types';

export interface CredentialsListResponse {
  credentials: Array<{
    service_name: string;
    base_url: string;
    auth_type: AuthType;
    created_at: string;
  }>;
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
