import { API, json, mutatingFetch, text, throwIfNotOk } from './_core';
import { changeFileUrl } from './fileUrls';
import type { DiffFile, RepoDiff } from '../../store/store';


export type ChangeStatus = 'pending' | 'set_aside' | 'applied' | 'discarded' | 'reverted' | 'withdrawn';

export interface Change {
  id: string;
  request_id: string;
  thread_id: string | null;
  thread_title: string | null;
  branch_name: string;
  repo_root: string;
  description: string;
  file_count: number;
  files: string[];
  requires_restart: boolean;
  hardened: boolean;
  status: ChangeStatus;
  created_at: string;
  resolved_at: string | null;
  pre_merge_sha: string | null;
  post_merge_sha: string | null;
  commits: string[];
  summary: string | null;
  incomplete: boolean;
  thread_unsettled?: boolean;
  thread_settling?: boolean;
  resolving_conflict?: boolean;
  apply_phase_started_at?: string | null;
  predicted_conflict?: ConflictPrediction | null;
  needs_hardening: boolean;
  apply_ready: boolean;
}

export type ConflictPrediction = 'unknown' | 'clean' | 'conflict';

export interface PhaseEstimate {
  typical_secs: number;
  runs: number;
}

export interface ApplyEstimates {
  hardening: PhaseEstimate | null;
  resolving_conflict: PhaseEstimate | null;
}

export type LaneStep = 'resolving' | 'landing';

export interface LaneOpenBatch {
  batch_id: string;
  lane_thread_id: string;
  members: string[];
  step: LaneStep;
}

export type LandingSetting = 'here' | 'another-host';

export interface LandingLane {
  setting?: LandingSetting;
  open_batch: LaneOpenBatch | null;
  waiting: string[];
  multiple_landers?: boolean;
  remote_members?: { change_id: string; title: string; gateway: string | null }[];
  gathering: boolean;
  waiting_for_origin?: string;
  arrival_hold?: { batch_id: string; arrivals: string[] };
}

export type MainCheckState = 'idle' | 'running' | 'passed' | 'red' | 'unavailable';

export type MainCheckSetting = 'here' | 'another-host' | 'off';

export interface MainCheck {
  state: MainCheckState;
  setting?: MainCheckSetting;
  repair?: {
    state: 'waiting' | 'running' | 'none';
    fix_thread_id?: string;
    started_by?: 'auto' | 'fix-now';
    waiting_on: string[];
  };
  failures?: Array<{ suite: string; name: string; detail: string }>;
  flaky?: Array<{ suite: string; name: string; detail: string }>;
  check_id?: string;
  from_sha?: string;
  to_sha?: string;
  run_url?: string;
  fix_thread_id?: string;
  flaky_fix_thread_id?: string;
  summary?: string;
}

export interface ApiRestartGroup {
  thread_id: string | null;
  thread_title: string | null;
  commits: string[];
}

export interface ChangesState {
  pending: Change[];
  set_aside?: Change[];
  applied: Change[];
  total_pending: number;
  restart_required: boolean;
  restart_groups: ApiRestartGroup[];
  client_update_available: boolean;
  has_more_applied: boolean;
  apply_all_in_progress: boolean;
  apply_all_batch?: {
    change_ids: string[];
    resolved_change_ids: string[];
    applying_change_ids?: string[];
    resolving_change_ids?: string[];
  } | null;
  standing_apply_thread_ids?: string[];
  apply_estimates?: ApplyEstimates;
  landing_lane?: LandingLane;
  main_check?: MainCheck;
}

export async function fetchChanges(params?: {
  limit?: number;
  before?: number;
}): Promise<ChangesState> {
  const qs = new URLSearchParams();
  if (params?.limit != null) qs.set('limit', String(params.limit));
  if (params?.before != null) qs.set('before', String(params.before));
  const q = qs.toString();
  return json(`${API}/changes${q ? `?${q}` : ''}`);
}

export type ApplyStatus = 'applied' | 'noop' | 'hardening' | 'conflict' | 'queued';

export interface ApplyChangeResult {
  status: ApplyStatus;
  change_id: string;
  thread_id: string | null;
  message: string;
  restart_required: boolean;
  version_build?: 'started' | 'deferred' | 'none';
  deferred_coding_threads?: number;
  applied_commit?: string;
  previous_commit?: string;
  commits_applied: number;
  files_changed: number;
  conflict_thread_id?: string;
  review_thread_id?: string;
}

const APPLY_TIMEOUT_MS = 600_000;

export async function applyChange(id: string): Promise<ApplyChangeResult> {
  return json(`${API}/changes/${id}/apply`, { method: 'POST' }, APPLY_TIMEOUT_MS);
}

export async function discardChange(id: string): Promise<{ message: string }> {
  return json(`${API}/changes/${id}/discard`, { method: 'POST' });
}

export async function setAsideChange(id: string): Promise<{ message: string }> {
  return json(`${API}/changes/${id}/set-aside`, { method: 'POST' });
}

export async function bringBackChange(id: string): Promise<{ message: string }> {
  return json(`${API}/changes/${id}/bring-back`, { method: 'POST' });
}

export interface ApplyAllResult {
  message: string;
  restart_required?: boolean;
  status?: ApplyStatus;
  batch_id?: string;
  batch_size?: number;
  conflict_thread_id?: string;
  review_thread_id?: string;
  applied?: number;
  failed?: number;
}

export async function applyAllChanges(): Promise<ApplyAllResult> {
  return json(`${API}/changes/apply-all`, { method: 'POST' }, APPLY_TIMEOUT_MS);
}

export async function armStandingApply(
  threadId: string,
  changeId?: string,
): Promise<{ message: string }> {
  return json(`${API}/standing-applies`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ thread_id: threadId, change_id: changeId ?? null }),
  });
}

export async function disarmStandingApply(threadId: string): Promise<{ message: string }> {
  return json(`${API}/standing-applies/${encodeURIComponent(threadId)}`, { method: 'DELETE' });
}

export async function cancelApplyAllChanges(): Promise<{
  canceled_batches: number;
  disarmed: number;
}> {
  return json(`${API}/changes/apply-all/cancel`, { method: 'POST' });
}

export async function discardAllChanges(): Promise<{ discarded: number; failed: number; errors: string[] }> {
  return json(`${API}/changes/discard-all`, { method: 'POST' });
}

export async function revertChange(id: string): Promise<{ message: string }> {
  return json(`${API}/changes/${id}/revert`, { method: 'POST' });
}

export interface RepoChangesState {
  pending: Change[];
  applied: Change[];
  has_more: boolean;
}

export async function getChangeById(changeId: string): Promise<Change> {
  return json(`${API}/changes/${changeId}`);
}

export async function getChangeDiff(changeId: string): Promise<RepoDiff> {
  return json(`${API}/changes/${changeId}/diff`);
}

export interface ThreadCcDiff {
  files: DiffFile[];
  repo_root: string;
  branch_name: string;
  base_ref: string;
}

export async function getThreadCcDiff(threadId: string): Promise<ThreadCcDiff> {
  return json(`${API}/threads/${encodeURIComponent(threadId)}/cc-diff`);
}

export async function getChangeFileContent(changeId: string, path: string): Promise<string> {
  return text(changeFileUrl(changeId, path));
}

export async function getRepoChanges(repoId: string, limit?: number, before?: number): Promise<RepoChangesState> {
  const params = new URLSearchParams();
  if (limit != null) params.set('limit', String(limit));
  if (before != null) params.set('before', String(before));
  const qs = params.toString();
  return json(`${API}/changes/for-repo/${encodeURIComponent(repoId)}${qs ? `?${qs}` : ''}`);
}

export async function restartEngine(): Promise<void> {
  const res = await mutatingFetch(`${API}/restart`, { method: 'POST' });
  await throwIfNotOk(res);
}

export interface DeferredBuild {
  coding_threads: number;
  threads: { id: string; title: string; state: string }[];
  deferred_ms: number;
}

export interface EngineVersionStatus {
  build_id: string;
  update_available: boolean;
  disk_build_id?: string;
  packaged: boolean;
  build_state: 'idle' | 'building' | 'verifying' | 'ready' | 'failed' | 'deferred';
  build_deferred?: DeferredBuild;
  source_behind_head?: boolean;
  head_commit?: string;
  build_failure?: BuildFailure;
  shared_build_in_progress?: boolean;
  build_elapsed_ms?: number;
  build_queued?: QueuedBuild;
  pending_commits?: PendingCommits;
  frontend_refresh_elapsed_ms?: number;
  frontend_build_watch_stopped?: { since_ms: number; cause: string | null };
}

export interface QueuedBuild {
  holders: string[];
}

export interface BuildFailure {
  summary: string;
  remedy?: string;
  kind?: 'not-delivered' | 'unsupported-launch';
  repeatable: boolean;
}

export type CommitGroupKind = 'new' | 'fixed' | 'improved' | 'other' | 'housekeeping';

export interface CommitGroup {
  kind: CommitGroupKind;
  total: number;
  descriptions: string[];
}

export interface PendingCommits {
  total: number;
  groups: CommitGroup[];
}

export async function engineVersionStatus(): Promise<EngineVersionStatus> {
  return json(`${API}/engine/version-status`);
}

export interface ChangelogRelease {
  version: string;
  date?: string | null;
  notes: string;
}

export async function engineChangelog(): Promise<ChangelogRelease[]> {
  const body = await json<{ releases: ChangelogRelease[] }>(`${API}/engine/changelog`);
  return body.releases;
}

export async function rebuildEngine(mode: 'join' | 'restart' = 'restart'): Promise<void> {
  const query = mode === 'join' ? '?mode=join' : '';
  const res = await mutatingFetch(`${API}/engine/rebuild${query}`, { method: 'POST' });
  await throwIfNotOk(res);
}

export function getEngineBuildLog(signal?: AbortSignal): Promise<string> {
  return text(`${API}/engine/build-log`, { signal, credentials: 'same-origin' });
}
