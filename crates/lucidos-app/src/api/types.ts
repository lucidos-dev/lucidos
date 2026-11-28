
import type { ActorMode } from '../store/thread-events';
import type { CredentialInfo, CredentialRequest } from '../store/types';
import type { CodingAgent, IntelligenceRung } from '../generated/thread-event-wire';

export type { CodingAgent, IntelligenceRung } from '../generated/thread-event-wire';

export interface ChatRequestBody {
  message: string;
  hook_args?: Record<string, string>;
  mode: ActorMode;
  model?: string;
  device_id?: string;
  reasoning_effort?: string;
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
  image_hashes?: string[];
  images?: Array<{ base64: string; mime_type?: string }>;
  use_coding_agent?: boolean;
  cc_model?: string;
  coding_agent?: CodingAgent;
  event_id?: string;
  thread_id?: string;
  input_chain?: string;
  compose_epoch?: number;
  input_seq?: number;
  new_thread?: boolean;
  mentions?: string[];
  agent_mentioned?: boolean;
  parent_thread_id?: string;
  spawning_event_id?: string;
  conflict_change_id?: string;
  repo_id?: string;
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

export interface AgentBinaryStatus {
  path: string | null;
  source: 'override' | 'detected' | 'path' | 'not-found';
  valid: boolean;
  error?: string;
  version?: string;
}

export interface AgentBinariesResponse {
  claude_code: AgentBinaryStatus;
  codex: AgentBinaryStatus;
}

export interface Band {
  multiple: number;
  label: string;
}

export interface ServedIntelligenceRung extends IntelligenceRung {
  label: string;
  on: boolean;
  estimated: boolean;
  intelligence: Band | null;
  cost: Band | null;
  time: Band | null;
  is_default: boolean;
}

export interface LongPromptShare {
  effective: number;
  set: number | null;
  measured: number | null;
  line_tokens: number;
}

export type LadderSurface = 'chat' | CodingAgent;

export interface IntelligenceLadder {
  other_backend_ladders?: CodingAgentLadder[];
  surface: LadderSurface;
  coding_agent?: CodingAgent;
  source: string;
  estimated: string;
  customised: boolean;
  climbs: boolean;
  shipped_rungs: IntelligenceRung[];
  default_rung: IntelligenceRung | null;
  reference: IntelligenceRung | null;
  long_prompt_share: LongPromptShare | null;
  rungs: ServedIntelligenceRung[];
}

export interface CodingAgentLadder extends IntelligenceLadder {
  coding_agent: CodingAgent;
  long_prompt_share: LongPromptShare;
}

export interface RouteInfo {
  provider: string;
  id: string;
  context_window?: number;
  reasoning_efforts: string[];
}

export interface ModelInfo {
  id: string;
  label: string;
  routes: RouteInfo[];
  preferred_provider: string | null;
  vision: boolean;
  default_effort: string | null;
  successor: string | null;
  sort_order: number;
  source: string;
  on_chat_ladder: boolean;
  created_at: string;
}

export interface ModelsListResponse {
  models: ModelInfo[];
}

export interface ResponseStyle {
  id: string;
  label: string;
  description: string;
  instruction: string;
  source: 'builtin' | 'overridden' | 'user';
  editable: boolean;
}

export interface ResponseStylesListResponse {
  styles: ResponseStyle[];
}

export interface CronPreview {
  next_runs: string[];
  warnings: string[];
}

export interface ApiResult {
  success: boolean;
  error?: string;
  credential_request?: CredentialRequest;
  auth_url?: string;
  cron_preview?: CronPreview;
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

export interface TriggerRunResult {
  success: boolean;
  status?: 'started' | 'queued' | 'already-running';
  message: string;
}

export interface DeviceInfo {
  id: string;
  name: string | null;
  pairing_label: string | null;
  user_agent: string | null;
  push_enabled: boolean;
  last_seen_at: string;
  created_at: string;
  replaced_ids?: string[];
}


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

export type EmbeddingModelLoadState =
  | { kind: 'downloading'; downloaded_bytes: number; total_bytes: number }
  | { kind: 'loading' }
  | { kind: 'ready' }
  | { kind: 'waiting'; attempt: number }
  | { kind: 'failed'; message: string };

export interface EmbeddingModelStatus {
  model_id: string;
  load_state: EmbeddingModelLoadState;
}

export interface BackfillProgress {
  done: number;
  total: number;
  done_milli: number;
  nodes_done: number;
  nodes_total: number;
  waiting_for_model: boolean;
  retrying: boolean;
  ready: boolean;
  spent_usd: number | null;
  cap_usd: number | null;
  next_cap_usd: number | null;
  held: boolean;
  failing_model?: string | null;
  spend_cap_held_until?: string | null;
  spend_cap_resets_at?: string | null;
  seq: number;
}

export type TreeBackfill =
  | { state: 'off'; started: boolean }
  | { state: 'running'; progress: BackfillProgress }
  | { state: 'ready'; filling?: BackfillProgress };

export interface RecallLine {
  id: string;
  text: string;
  sender_device_id?: string;
}

export interface SummaryTreeTop {
  entries: number;
  lines: RecallLine[];
}

export interface RecallZoomResponse {
  id: string;
  lines: RecallLine[];
}

export interface RecallDateResponse {
  id: string;
  from: string;
  to: string;
}

export interface SummaryTreeThread {
  thread_id: string;
  title: string | null;
  last_activity: string;
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

export interface EffortCost {
  effort: string;
  usable_secs: Bounds;
  complete_secs: Bounds;
  backfill_usd: Bounds;
  backfill_usd_central: number;
  daily_usd: Bounds;
  daily_usd_central: number;
  turn_usd: Bounds;
  turn_usd_central: number;
  cap_usd: number;
}

export interface TreeBackfillEstimate {
  calls: Bounds;
  usable_secs: Bounds;
  complete_secs: Bounds;
  daily_calls: Bounds;
  turn_calls: Bounds;
  costs: { model: string; by_effort: EffortCost[] }[];
}

export interface BackgroundModel {
  model: string;
  effort: string | null;
  source: 'preference' | 'default' | 'chat-model';
  reachable: boolean;
  generation: GenerationStep | null;
  not_served: string[];
  needs_vision: boolean;
  vision: boolean;
  asks_no_model: boolean;
  unresolved: string | null;
  recommended: RecommendedSelection[];
}

export type GenerationStep =
  | { kind: 'successor'; model: string }
  | { kind: 'predecessor'; model: string }
  | { kind: 'held'; successor: string };

export interface RecommendedSelection {
  model: string;
  effort: string | null;
}

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
