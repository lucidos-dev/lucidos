import { API, json, mutatingFetch, text, throwIfNotOk } from './_core';
import type { DiffFile, RepoDiff } from '../../store/store';

// --- Changes ---

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
  status: string;
  created_at: string;
  resolved_at: string | null;
  pre_merge_sha: string | null;
  post_merge_sha: string | null;
  commits: string[];
  /** True when the originating CC turn ended in `ResponseFailed` — the
   * worktree state reflects partial work, not a deliberate completion.
   * `WaitingBanner` reads this to confirm before Apply so the user knows
   * they're about to land partial changes from a failed run. */
  incomplete: boolean;
  /** True when the originating thread is mid-turn (the coding agent is Running
   * or WaitingForUserAnswer). Applying then races the session's next proposal,
   * so the changes view disables Apply and drops it from Apply All. Defaults to
   * false (absent on non-pending changes and older payloads). */
  thread_active?: boolean;
}

/** One thread's contribution to the current restart-required toast: derived
 *  server-side from applied-but-not-yet-restarted changes since engine start. */
export interface ApiRestartGroup {
  thread_id: string | null;
  thread_title: string | null;
  commits: string[];
}

export interface ChangesState {
  pending: Change[];
  applied: Change[];
  total_pending: number;
  restart_required: boolean;
  restart_groups: ApiRestartGroup[];
  client_update_available: boolean;
  has_more_applied: boolean;
  /** True when an Apply All batch is live on the engine. Lets a reloaded page
   *  rehydrate the sticky "Applying changes…" toast — the driving
   *  `applyAllInProgress` signal resets on reload and the ApplyAllBatch* SSE
   *  events aren't replayed. */
  apply_all_in_progress: boolean;
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

export type ApplyStatus = 'applied' | 'noop' | 'hardening' | 'conflict';

/** Response body for POST /api/v1/changes/:id/apply. Mirrors the Rust
 *  `ApplyResult` struct in `crates/lucidos-engine/src/engine/types.rs`. */
export interface ApplyChangeResult {
  status: ApplyStatus;
  change_id: string;
  thread_id: string | null;
  message: string;
  restart_required: boolean;
  /** SHA of main HEAD after the merge. Only set when a real merge
   *  happened (absent for the external-repo handoff and non-`applied`). */
  applied_commit?: string;
  previous_commit?: string;
  commits_applied: number;
  files_changed: number;
  /** Set when `status === 'conflict'` — focus this thread to resolve. */
  conflict_thread_id?: string;
  /** Set when `status === 'hardening'` — track as "applying" until done. */
  review_thread_id?: string;
}

/** Apply spawns hardening / conflict Claude Code sessions; Apply All loops synchronously
 *  over N changes — both blow past the default 10s client timeout while the
 *  backend keeps working, surfacing a misleading abort. */
const APPLY_TIMEOUT_MS = 600_000;

export async function applyChange(id: string): Promise<ApplyChangeResult> {
  return json(`${API}/changes/${id}/apply`, { method: 'POST' }, APPLY_TIMEOUT_MS);
}

export async function discardChange(id: string): Promise<{ message: string }> {
  return json(`${API}/changes/${id}/discard`, { method: 'POST' });
}

export interface ApplyAllResult {
  message: string;
  restart_required?: boolean;
  /** Status of the FIRST change (applied synchronously so the HTTP response
   *  carries an immediate result). The remaining members are driven in the
   *  background — watch the ApplyAllBatch* SSE events for their resolution.
   *  Absent on the early-error branch. */
  status?: ApplyStatus;
  /** Engine-assigned batch id, present whenever a batch was started. */
  batch_id?: string;
  /** Set when the first change stopped at a conflict — same shape as ApplyChangeResult. */
  conflict_thread_id?: string;
  /** Set when the first change needs hardening — the recovery thread that will
   *  auto-apply it once `/harden` completes. */
  review_thread_id?: string;
  applied?: number;
  failed?: number;
}

export async function applyAllChanges(): Promise<ApplyAllResult> {
  return json(`${API}/changes/apply-all`, { method: 'POST' }, APPLY_TIMEOUT_MS);
}

/** Cancel the running Apply All batch — stops the driver, interrupts the
 *  in-flight hardening/merge, and leaves not-yet-applied changes pending. The
 *  resulting ApplyAllBatchCompleted SSE clears the in-progress state. */
export async function cancelApplyAllChanges(): Promise<{ canceled_batches: number }> {
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

/** 3-dot diff of a CC worktree branch vs the repo's default remote branch.
 *  Used for external-repo Claude Code sessions that never produce a Lucidos `Change`. */
export async function getThreadCcDiff(threadId: string): Promise<ThreadCcDiff> {
  return json(`${API}/threads/${encodeURIComponent(threadId)}/cc-diff`);
}

/** URL of the full "after" version of a file in a change. Served with a
 *  content-type inferred from the extension, so a binary-media preview can
 *  point an <img>/<video>/<audio>/<iframe> `src` at it. */
export function changeFileUrl(changeId: string, path: string): string {
  const params = new URLSearchParams({ path });
  return `${API}/changes/${encodeURIComponent(changeId)}/file?${params}`;
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

/** Whether a newer engine version is ready to switch onto, plus the dev
 *  background-rebuild state. Dev half of the "New version available → Switch to
 *  new version" flow; packaged reports `update_available: false` (its new-version
 *  source is the release updater). See engine `GET /api/v1/engine/version-status`. */
export interface EngineVersionStatus {
  build_id: string;
  update_available: boolean;
  /** The on-disk binary's build id (dev), omitted when packaged / unreadable.
   *  The switch badge+toast dismissal is keyed on this so a dismiss sticks for
   *  THIS on-disk build but a genuinely newer build re-surfaces the switch. */
  disk_build_id?: string;
  packaged: boolean;
  build_state: 'idle' | 'building' | 'ready' | 'failed';
  /** Dev only: the engine SOURCE is behind HEAD by a restart-requiring change —
   *  a NEW engine version exists in source even if no fresh binary is on disk yet
   *  (rebuild failed / not run). Distinct from `update_available` (a fresh binary
   *  IS on disk). Lets the UI surface a pending version + offer "Rebuild & Switch"
   *  so the Switch is never a dead-end. Absent/false when packaged or git is
   *  unavailable. */
  source_behind_head?: boolean;
}

export async function engineVersionStatus(): Promise<EngineVersionStatus> {
  return json(`${API}/engine/version-status`);
}

/** Manually kick off the dev background engine rebuild (escape hatch for a wedged
 *  workspace whose source is behind HEAD with a stale binary — e.g. after a failed
 *  background rebuild). No-op packaged. The resulting version-status `build_state`
 *  transitions drive the UI (building → ready → Switch). See engine
 *  `POST /api/v1/engine/rebuild`. */
export async function rebuildEngine(): Promise<void> {
  const res = await mutatingFetch(`${API}/engine/rebuild`, { method: 'POST' });
  await throwIfNotOk(res);
}
