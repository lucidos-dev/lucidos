import { API, json, mutatingFetch, text, throwIfNotOk } from './_core';
import { changeFileUrl } from './fileUrls';
import type { DiffFile, RepoDiff } from '../../store/store';

// --- Changes ---

/** Mirrors the engine's `ChangeStatus`, the five values `changes.status` holds. */
export type ChangeStatus = 'pending' | 'set_aside' | 'applied' | 'discarded' | 'reverted';

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
  /** The *change summary*: one model-written line for a change of several
   *  commits. Null for a single commit and until it lands. Read it through
   *  `changeHeadline`, never on its own. */
  summary: string | null;
  /** True when the work did not come from a finished turn: a user Stop cut it
   *  short, or recovery found it after a turn was killed. Apply confirms
   *  first, and Apply All passes it over. */
  incomplete: boolean;
  /** True when the originating thread has not finished with this change: it is
   * mid-turn (Running or WaitingForUserAnswer) or holds a live event wait.
   * Applying then races the session's next proposal, so the changes view draws
   * no Apply and Apply All drops it. A thread waiting only on its sub-threads is
   * NOT unsettled (ADR 0249). Defaults to false (absent on non-pending changes
   * and older payloads). */
  thread_unsettled?: boolean;
  /** True when the originating thread is *settling*: running, paused, or
   *  watching an event. The half of `thread_unsettled` a *standing apply* can
   *  wait through. A thread parked on a question is unsettled too, and arming
   *  one drops the moment it is pressed. Defaults to false. */
  thread_settling?: boolean;
  /** True while an apply of this change is resolving merge conflicts. The
   *  thread works only to finish that apply, and its completion lands the
   *  change, so no standing apply is offered. Defaults to false. */
  resolving_conflict?: boolean;
  /** When the slow phase in flight began: an open conflict resolution, or an
   *  open hardening. Null when neither runs, and on non-pending changes. */
  apply_phase_started_at?: string | null;
  /** Whether merging this change into `main` as it stands would conflict,
   *  from `git merge-tree`. `unknown` when git could not answer. Null on
   *  non-pending changes. */
  predicted_conflict?: ConflictPrediction | null;
}

/** Mirrors the engine's `ConflictPrediction`. */
export type ConflictPrediction = 'unknown' | 'clean' | 'conflict';

/** How long one slow apply phase usually takes in this workspace. */
export interface PhaseEstimate {
  typical_secs: number;
  runs: number;
}

/** Mirrors the engine's `ApplyEstimates`. A phase is null until enough runs
 *  exist to estimate it. */
export interface ApplyEstimates {
  hardening: PhaseEstimate | null;
  resolving_conflict: PhaseEstimate | null;
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
  /** Changes kept for later, newest first. Absent on an older payload. */
  set_aside?: Change[];
  applied: Change[];
  total_pending: number;
  restart_required: boolean;
  restart_groups: ApiRestartGroup[];
  client_update_available: boolean;
  has_more_applied: boolean;
  /** True when an Apply All batch is live on the engine. Lets a reloaded page
   *  bring back the Apply All row in the Lucidos menu: the driving
   *  `applyAllInProgress` signal resets on reload, and the ApplyAllBatch* SSE
   *  events aren't replayed. */
  apply_all_in_progress: boolean;
  /** The running batch's members in apply order and what each is doing, so a
   *  reload keeps "change N of M". Null while no batch runs, and briefly after
   *  an engine restart. Absent on an older payload, as are the two member
   *  lists on an engine from before ADR 0314. */
  apply_all_batch?: {
    change_ids: string[];
    resolved_change_ids: string[];
    applying_change_ids?: string[];
    resolving_change_ids?: string[];
  } | null;
  /** Threads carrying a *standing apply*. Keyed by thread, not by change: a
   *  sweep arms a thread that has proposed nothing yet, and its prompt row
   *  still renders the armed state. Absent on an older payload. */
  standing_apply_thread_ids?: string[];
  /** How long hardening and conflict resolution usually take here. Absent on
   *  an older payload. */
  apply_estimates?: ApplyEstimates;
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

/** Keep a pending change for later, out of Review and Apply All. */
export async function setAsideChange(id: string): Promise<{ message: string }> {
  return json(`${API}/changes/${id}/set-aside`, { method: 'POST' });
}

/** Return a set-aside change to pending. */
export async function bringBackChange(id: string): Promise<{ message: string }> {
  return json(`${API}/changes/${id}/bring-back`, { method: 'POST' });
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
  /** How many changes the batch holds. */
  batch_size?: number;
  /** Set when the first change stopped at a conflict — same shape as ApplyChangeResult. */
  conflict_thread_id?: string;
  /** Set when the first change needs hardening — the recovery thread that will
   *  auto-apply it once `/harden` completes. */
  review_thread_id?: string;
  applied?: number;
  failed?: number;
}

/** Apply every pending change whose thread has settled. */
export async function applyAllChanges(): Promise<ApplyAllResult> {
  return json(`${API}/changes/apply-all`, { method: 'POST' }, APPLY_TIMEOUT_MS);
}

/** Arm a standing apply: the thread's change applies once it settles.
 *
 *  Omit `changeId` for a thread that has proposed nothing yet, and the arm
 *  takes whatever it proposes. */
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

/** Take the instruction back. */
export async function disarmStandingApply(threadId: string): Promise<{ message: string }> {
  return json(`${API}/standing-applies/${encodeURIComponent(threadId)}`, { method: 'DELETE' });
}

/** Cancel the running Apply All batch — stops the driver, interrupts the
 *  in-flight hardening/merge, and leaves not-yet-applied changes pending. The
 *  resulting ApplyAllBatchCompleted SSE clears the in-progress state. */
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

/** 3-dot diff of a CC worktree branch vs the repo's default remote branch.
 *  Used for external-repo Claude Code sessions that never produce a Lucidos `Change`. */
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
  /** Dev only: the checkout's HEAD commit, present only while something is
   *  pending. **Identity, never display**: it is what a dismissal of the
   *  *pending* version toast is pinned to, the way the Switch toast's dismissal
   *  is pinned to `disk_build_id`. A pending version has no on-disk build to
   *  name (that absence is what makes it pending), so without this the toast
   *  had nothing to remember and could not be dismissed at all. */
  head_commit?: string;
  /** Dev only: a rebuild for this HEAD already completed and produced nothing
   *  switchable, so pressing *Rebuild* runs the same build from the same source
   *  and lands right back here. The UI withholds the button and names the
   *  operator fix instead. Absent/false when packaged. */
  rebuild_wedged?: boolean;
  /** Dev only: why the last build failed, present only with
   *  `build_state: 'failed'`. The toast renders this INSTEAD of pointing at the
   *  engine log, which is unreachable on the phone the toast is often read on.
   *
   *  The engine always sends it with a failure. An older engine may not, and
   *  the toast then says the cause was not reported, rather than dressing one
   *  up. */
  build_failure?: BuildFailure;
  /** Dev only: the checkout-shared engine-build lock is currently held — a
   *  background rebuild of the shared binary is in flight (a co-located peer
   *  workspace's build OR this engine's own). Lets a workspace that lost the
   *  shared lock show the building spinner instead of the manual "Rebuild"
   *  escape hatch. Absent/false when packaged. */
  shared_build_in_progress?: boolean;
  /** How long THIS engine's own background rebuild has been running, in ms.
   *  Absent when no build of ours is in flight, which includes a co-located
   *  peer's build (we don't have its clock) and packaged.
   *
   *  ELAPSED rather than a start timestamp, deliberately: the status toast
   *  renders a live counter from it, and differencing an engine wall-clock
   *  against the browser's would show a wrong (or negative) duration whenever
   *  the two disagree. The client anchors this to its own `Date.now()` at
   *  receipt and counts up locally, so skew never reaches the number. */
  build_elapsed_ms?: number;
  /** Present while THIS engine's rebuild waits for a *build slot* instead of
   *  compiling. Absent while it compiles, when idle, for a co-located peer's
   *  build, and when packaged. */
  build_queued?: QueuedBuild;
  /** The commits a Switch would bring, grouped by what they are (see
   *  `PendingCommits` for which commits count). Two surfaces read it,
   *  the status toast while a rebuild runs and the new-version confirm once one
   *  is ready.
   *
   *  Absent means UNKNOWN (git couldn't answer, or nothing was asked because
   *  no surface would show it), never "none pending". A present object with
   *  `total: 0` is the only way to say there is nothing to bring. */
  pending_commits?: PendingCommits;
  /** How long THIS engine has waited for the build-watch to republish `dist/`
   *  after a frontend-only Apply, in ms. Absent when no such wait is running,
   *  when packaged, and on an engine too old to report it. Elapsed for the
   *  reason `build_elapsed_ms` gives. */
  frontend_refresh_elapsed_ms?: number;
}

/** A rebuild waiting for a build slot. `holders` names what holds each slot,
 *  by the label its wrapper recorded. */
export interface QueuedBuild {
  holders: string[];
}

/** Why a background build failed, reduced to what a toast can carry.
 *
 *  One line rather than a log tail, because this IS the message: the engine log
 *  it replaces cannot be opened from a phone. See engine `BuildFailure`. */
export interface BuildFailure {
  /** The first real error line from the build. For a build-script panic this is
   *  the panic message, not cargo's generic "failed to run custom build
   *  command", which says nothing a reader can use. */
  summary: string;
  /** The shell command that clears this failure, when the class is recognized
   *  and its fix is exact. Absent for an ordinary compile error, whose fix is
   *  to change the code. */
  remedy?: string;
  /** Retrying is PROVED futile, so the toast withholds Retry (the
   *  `rebuild_wedged` treatment for a failing build). Never inferred from
   *  silence: an unreadable or unrecognized failure is `false`, since retiring
   *  the button wrongly strands a build that would have succeeded. */
  repeatable: boolean;
}

/** Which bucket a pending commit falls in, from its conventional-commit type.
 *  The engine classifies; the frontend words it (`GROUP_LABEL` in
 *  `store/backgroundActivity.ts`).
 *
 *  `housekeeping` (docs/chore/test/ci/build/harden) is COUNTED, never listed:
 *  its `descriptions` is always empty. */
export type CommitGroupKind = 'new' | 'fixed' | 'improved' | 'other' | 'housekeeping';

/** One bucket of the pending range, with its own count so a capped list can say
 *  how much it is not showing. */
export interface CommitGroup {
  kind: CommitGroupKind;
  /** Every commit in this group, capped or not. */
  total: number;
  /** Newest-first, capped engine-side, and empty for `housekeeping`. Each line
   *  is the commit subject with its conventional-commit TYPE stripped and its
   *  scope kept as a lead-in (`ui: the trash is sized by its ink`). */
  descriptions: string[];
}

/** The commits a Switch would bring, since the running engine's commit. Two
 *  kinds are excluded engine-side:
 *  - Merges: an Apply lands as a merge whose subject is a branch name, and what
 *    it merged is already in the range under its own subject.
 *  - Commits the served client already carries: the engine serves those
 *    frontend-only changes without a restart.
 *
 *  See `EngineVersionStatus.pending_commits` for why an ABSENT value and a
 *  `total: 0` one mean different things. */
export interface PendingCommits {
  /** Every commit the switch brings, including the ones no group lists.
   *  Equals the sum of the group totals (the engine derives it from them). */
  total: number;
  /** Non-empty groups, in the order the toast lists them. */
  groups: CommitGroup[];
}

export async function engineVersionStatus(): Promise<EngineVersionStatus> {
  return json(`${API}/engine/version-status`);
}

/** One published release, as the What's New panel shows it. Comes from the
 *  changelog baked into the engine binary, so this is the history you HAVE:
 *  the notes for a release the updater is OFFERING postdate that binary and
 *  arrive with the update check instead (`AppUpdateOffer.notes`). */
export interface ChangelogRelease {
  /** No leading `v`, e.g. `0.26.3`. Matched against the `release` from /health
   *  to mark the one you are running. */
  version: string;
  /** As written in the heading, or absent when the heading carries only a
   *  version. */
  date?: string | null;
  /** The section body as raw markdown, heading excluded. Rendered client-side
   *  (`utils/renderMarkdown.ts`). */
  notes: string;
}

/** Every published release, newest first. See engine
 *  `GET /api/v1/engine/changelog`. */
export async function engineChangelog(): Promise<ChangelogRelease[]> {
  const body = await json<{ releases: ChangelogRelease[] }>(`${API}/engine/changelog`);
  return body.releases;
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
