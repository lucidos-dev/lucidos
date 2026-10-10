/**
 * Background activity: long-running work surfaced on the brand badge and in
 * the Lucidos menu's activity group, where its row unfolds to the detail.
 *
 * Four activities today:
 *
 * - a dev engine rebuild (`engineBuilding`, which already drove the spinning
 *   badge);
 * - the frontend rebuild after a frontend-only Apply (`frontendRefreshDetail`);
 * - the embedding-model download, which until now ran completely invisibly: the
 *   ~465 MB first-run fetch takes minutes, during which memory search,
 *   extraction and semantic thread search are dead;
 * - an **Expose** run (`tailscale serve`), which the user DOES start, and which
 *   can legitimately spend minutes waiting for a tailnet approval.
 *
 * That last one stretches "work the user did not start", and belongs here
 * anyway: it is long-running, it outlives the pane that launched it, and the
 * badge is the one place in the app that says something is happening. The
 * difference it does make is in the toast rule, which lives in
 * `actions/backgroundActivity.ts`: an Expose run narrates in a toast, because
 * its approval link must reach the user, while every other job is told only in
 * the menu.
 *
 * Everything here is a PURE function of explicit arguments, with the signal
 * reads left to the callsite (the `refreshRowState` precedent in
 * WorkspaceMenuRows.tsx), so the derivation and its copy are unit-testable
 * without a render. Actions are carried as DESCRIPTORS rather than callbacks for
 * the same reason: a closure in here would be the end of that.
 */

import { signal } from '@preact/signals';
import type { EmbeddingModelStatus } from '../api/types';
import type { TailscaleServeProgress } from '../utils/tauri';
import type { CommitGroup, CommitGroupKind, PendingCommits } from '../api/client';
import { formatBytes } from '../utils/formatBytes';
import { formatElapsed } from '../utils/formatTime';

/** Last known embedding-model status, or `null` before the first read.
 *
 *  Filled two ways, and it needs both: the startup/resume snapshot
 *  (`/memory/embedding-model-status`) for a client that connected mid-download,
 *  and the `EmbeddingModelStatusChanged` SSE frame for everything after. On a
 *  fresh workspace the download begins at engine boot, seconds before the app
 *  exists, so the snapshot is what makes the first reading correct.
 *
 *  A nullable signal rather than `Loadable<T>`, matching the other SSE-driven
 *  status feeds (`memoryRebuildProgress`, `backupProgress`, `recoveryProgress`).
 *  `Loadable` governs a view's async data source, where "loading" and "failed"
 *  must look different from "empty"; here `null` means "nothing known yet",
 *  which correctly renders as no indicator, and a failed snapshot read is
 *  best-effort telemetry that the next frame supersedes. See
 *  `docs/code-review-priors.md`. */
export const embeddingModelStatus = signal<EmbeddingModelStatus | null>(null);

/** The latest frame of the in-flight Expose run, or `null` when none is running.
 *
 *  Fed only by the `tailscale-serve-progress` Tauri event, and cleared on a
 *  terminal frame. It is the run's single source of truth, which is why the
 *  Mobile Access page reads it for its button state rather than keeping a local
 *  `busy` flag: a run outlives the pane, and a page-local flag would be lost the
 *  moment the user navigated away and back.
 *
 *  Nullable rather than `Loadable<T>`, matching the other event-driven feeds
 *  beside it: `null` means "no run", which correctly renders as no indicator. */
export const tailscaleServeRun = signal<TailscaleServeProgress | null>(null);

/** How long a build has been running, as the engine last reported it.
 *
 *  The odd one out among the three feeds: the other two are PUSHED a frame per
 *  update (`EmbeddingModelStatusChanged` over SSE, `tailscale-serve-progress`
 *  over Tauri), so re-rendering per frame keeps them current. A build emits only
 *  its transitions, so this arrives on the ~4s version-status poll and the
 *  seconds in between are counted locally. Hence `anchoredAt`. */
export interface ElapsedAnchor {
  /** Build age in ms as the ENGINE measured it, at the moment `anchoredAt` was
   *  taken. `null` when the engine reported none, which is the co-located peer's
   *  build: the badge spins for it, but its clock is not ours to read. */
  elapsedMs: number | null;
  /** The client's own `Date.now()` when `elapsedMs` arrived. The live counter is
   *  `elapsedMs + (now - anchoredAt)`, so it advances between polls without ever
   *  differencing the engine's wall clock against the browser's. Two clocks that
   *  disagree would otherwise show a wrong, possibly negative, build age. */
  anchoredAt: number;
}

export interface EngineBuildDetail extends ElapsedAnchor {
  /** The commits this build will bring, or `null` when git couldn't say. Those
   *  are different answers: `{ total: 0 }` is "nothing pending", `null` is "we
   *  don't know", and only the first may be stated out loud. */
  pendingCommits: PendingCommits | null;
  /** What holds every build slot while this build waits for one. Absent while
   *  it compiles, and for a peer's build, whose wait we cannot see. */
  queuedBehind?: readonly string[];
}

/** Latest build narration, or `null` when no rebuild is in flight. Written only
 *  by `setEngineBuilding` (`store/actions/engine-update.ts`), paired with
 *  `engineBuilding`, so a cleared boolean can never leave a stale timer behind.
 *
 *  Nullable rather than `Loadable<T>`, matching the two feeds above. */
export const engineBuildDetail = signal<EngineBuildDetail | null>(null);

/** How long the engine has waited for the frontend rebuild after a
 *  frontend-only Apply, or `null` when it is not waiting. Written only by
 *  `pollEngineVersion` (`store/actions/engine-update.ts`), from
 *  `frontend_refresh_elapsed_ms`. */
export const frontendRefreshDetail = signal<ElapsedAnchor | null>(null);

/** Something a job's detail offers to DO about it, as data.
 *
 *  A descriptor, not a callback, so the derivation below stays pure and
 *  testable without a render. `actions/backgroundActivity.ts` maps each kind
 *  onto its real handler at the one place that already has them. */
export type ActivityAction =
  /** Open a URL in the system browser. Only ever a URL Rust already vetted. */
  | { kind: 'open-url'; label: string; url: string }
  /** Abandon the in-flight Expose run. */
  | { kind: 'cancel-tailscale-serve'; label: string };

export interface BackgroundActivity {
  kind: 'engine-build' | 'frontend-refresh' | 'embedding-model' | 'tailscale-serve';
  /** One line naming what is happening, e.g. "Downloading embedding model". */
  label: string;
  /** Appended after the label, e.g. "212 MB of 465 MB". */
  detail?: string;
  /** Fraction in [0, 1], or `null` when there is no honest percentage. */
  progress: number | null;
  /** A further line the detail shows under the label. */
  note?: string;
  /** Set while the work waits its turn instead of running: why it waits. The
   *  menu row and the brand badge then stop spinning and say Queued. */
  queued?: string;
  /** The primary thing to do about it, e.g. approve Serve for the tailnet. */
  action?: ActivityAction;
  /** The way out, e.g. cancelling a run that is waiting on the user. */
  secondaryAction?: ActivityAction;
}

/** What the detail says while the embedding model is still coming down.
 *
 *  Literally true, and the reason it is worth saying: an embed attempted before
 *  the model lands fails, and `index_memory_inner_impl` drops the item rather
 *  than storing it unindexed. The post-install sweep only re-embeds rows that
 *  already exist with a stale model id, so nothing created in this window is
 *  recovered without a manual memory rebuild (docs/known-gaps.md). */
export const MEMORY_NOT_INDEXED_NOTE =
  'You can keep working. Anything created before this finishes will not be searchable in memory.';

/** Background work currently in flight, in the order the menu lists it.
 *
 *  Only genuinely in-flight work counts, because this is what decides whether
 *  the badge spins:
 *
 *  - `downloading` qualifies. It is the minutes-long one this exists for.
 *  - `loading` does NOT. Building the ONNX session takes a few seconds on every
 *    single boot, warm cache included, and a spinner that flashes each time the
 *    app opens is noise rather than information.
 *  - `waiting` and `failed` do NOT. Neither is work in progress, and both
 *    already have a user-facing surface: the loader notifies after three failed
 *    attempts, and again if it gives up. An offline machine would otherwise
 *    spin the badge forever. A download this document watched still reports
 *    them once (see {@link embeddingModelOutcome}).
 *  - `ready` does NOT, obviously. */
export function backgroundActivities(
  engineBuilding: boolean,
  model: EmbeddingModelStatus | null,
  serveRun: TailscaleServeProgress | null = null,
  buildDetail: EngineBuildDetail | null = null,
  nowMs: number = Date.now(),
  frontendRefresh: ElapsedAnchor | null = null,
): BackgroundActivity[] {
  const activities: BackgroundActivity[] = [];
  if (engineBuilding) {
    const queuedBehind = buildDetail?.queuedBehind;
    activities.push({
      kind: 'engine-build',
      label: queuedBehind ? 'New version queued' : 'Building new version',
      // A cargo build reports no percentage, and inventing one would be worse
      // than the spinner the row falls back to. What it CAN say honestly is
      // how long it has been going and what it will bring.
      progress: null,
      detail: buildElapsedDetail(buildDetail, nowMs),
      note: 'You can switch to it once the build finishes.',
      queued: queuedBehind ? slotWaitReason(queuedBehind) : undefined,
    });
  }
  if (frontendRefresh) {
    // The engine is waiting for the build-watch to republish `dist/` after a
    // frontend-only Apply. The Refresh prompt follows once it lands.
    activities.push({
      kind: 'frontend-refresh',
      label: 'Building frontend',
      progress: null,
      detail: buildElapsedDetail(frontendRefresh, nowMs),
      note: 'A Refresh prompt appears once the build finishes.',
    });
  }
  const state = model?.load_state;
  if (state?.kind === 'downloading') {
    activities.push({
      kind: 'embedding-model',
      label: 'Downloading embedding model',
      detail: downloadDetail(state.downloaded_bytes, state.total_bytes),
      progress: downloadFraction(state.downloaded_bytes, state.total_bytes),
      note: MEMORY_NOT_INDEXED_NOTE,
    });
  }
  const serve = tailscaleServeActivity(serveRun);
  if (serve) activities.push(serve);
  return activities;
}

/** What the Expose run contributes while it is in flight, or `null` once it is
 *  over. The terminal phases are absent by design: the badge shows work IN
 *  FLIGHT, so a finished run must stop it spinning. Their outcome is
 *  {@link tailscaleServeOutcome}'s. */
export function tailscaleServeActivity(
  run: TailscaleServeProgress | null,
): BackgroundActivity | null {
  if (!run) return null;
  // Every step of this flow is indeterminate, so it spins throughout.
  const base = { kind: 'tailscale-serve', progress: null } as const;
  const cancel: ActivityAction = { kind: 'cancel-tailscale-serve', label: 'Cancel' };
  switch (run.phase) {
    case 'starting':
    case 'checking-tailnet':
      return { ...base, label: 'Setting up Tailscale access', secondaryAction: cancel };
    case 'configuring':
      return { ...base, label: 'Configuring tailscale serve', secondaryAction: cancel };
    case 'awaiting-tailnet-approval':
      // The one step that needs the user, and the whole reason this run is
      // worth narrating. Serve is a tailnet-level feature a tailnet admin turns
      // on in a browser, so there is nothing to do here but say so and hand over
      // the link the CLI printed. The run keeps waiting and finishes by itself.
      return {
        ...base,
        label: 'Waiting for you to enable Serve on your tailnet',
        note:
          'Tailscale needs Serve turned on for your tailnet before it can give this Mac an ' +
          'HTTPS address. Open the link, approve it, and setup continues on its own.',
        action: { kind: 'open-url', label: 'Enable in Tailscale', url: run.url },
        secondaryAction: cancel,
      };
    case 'waiting-for-https':
      return {
        ...base,
        label: 'Waiting for HTTPS to come up',
        note: 'The first certificate for this name can take a few seconds to provision.',
        secondaryAction: cancel,
      };
    case 'done':
    case 'failed':
    case 'cancelled':
      return null;
  }
}

/** Why a queued build waits, naming what holds the slots. */
function slotWaitReason(holders: readonly string[]): string {
  const list = holders.join(', ');
  return holders.length === 1
    ? `The one build slot is busy: ${list}. It takes it as soon as it frees up.`
    : `All ${holders.length} build slots are busy: ${list}. It takes the next one that frees up.`;
}

/** How long the build has been running, as the row's trailing detail, or
 *  `undefined` when there is no honest number.
 *
 *  Counted from the client anchor rather than the engine's clock, and re-derived
 *  on every call, which is what lets a 1s ticker advance it between the ~4s
 *  polls. A peer's build reports no elapsed at all (`elapsedMs === null`), and
 *  that stays blank instead of quietly timing from when THIS client noticed. */
function buildElapsedDetail(
  detail: ElapsedAnchor | null,
  nowMs: number,
): string | undefined {
  if (detail?.elapsedMs == null) return undefined;
  return formatElapsed(detail.elapsedMs + Math.max(0, nowMs - detail.anchoredAt));
}

/** What each commit group is called, in the build's detail and in the new-version
 *  confirm. ONE table, so the two surfaces cannot name a group differently.
 *
 *  The engine decides which bucket a commit is in, being the side that parses
 *  the log. The wording is the frontend's, like every other string here.
 *
 *  `housekeeping` has no heading because it is never a section: it is one
 *  counted line, worded by {@link housekeepingLine} below. */
export const GROUP_LABEL: Record<Exclude<CommitGroupKind, 'housekeeping'>, string> = {
  new: 'New',
  fixed: 'Fixed',
  improved: 'Improved',
  other: 'Other',
};

/** One group's items, with the tail the engine capped off named rather than
 *  dropped. A list that silently under-reports its own count is how a reader
 *  concludes a version is smaller than it is.
 *
 *  Shared with the new-version confirm, for the reason below. */
export function describedItems(group: CommitGroup): string[] {
  const hidden = group.total - group.descriptions.length;
  return hidden > 0 ? [...group.descriptions, `and ${hidden} more`] : [...group.descriptions];
}

/** How a pending range is counted, wherever it is counted.
 *
 *  Shared with the new-version confirm (`store/restartConfirmCopy.ts`), which
 *  describes the same range at the other end of the same build. The two must
 *  not phrase it differently, so neither owns the sentence. The count is what
 *  the switch adds (see `PendingCommits`). */
export function pendingCommitsHeadline(total: number): string {
  return `${total === 1 ? '1 commit' : `${total} commits`} ${comeWithTheNewVersion(total)}`;
}

/** The verb phrase after a count of `total` commits, shared with the
 *  new-version confirm's all-housekeeping intro. */
export function comeWithTheNewVersion(total: number): string {
  return total === 1 ? 'comes with the new version' : 'come with the new version';
}

/** The one counted line for the commits the detail does not describe. Names what
 *  is in the bucket rather than calling it "other", so the number reconciles
 *  with the heading without pretending the work was hidden. The parenthetical
 *  carries the contents in both forms, since "1 docs commit" does not read.
 *
 *  Shared with the new-version confirm, for the reason above. */
export function housekeepingLine(total: number): string {
  return total === 1
    ? '1 housekeeping commit (docs, tests, chores)'
    : `${total} housekeeping commits (docs, tests, chores)`;
}

/** "212 MB of 465 MB", or just the bytes so far when the total is not known
 *  yet (the first frame of a download can arrive before any file has declared
 *  its size). */
function downloadDetail(downloaded: number, total: number): string {
  return total > 0
    ? `${formatBytes(downloaded)} of ${formatBytes(total)}`
    : formatBytes(downloaded);
}

/** Determinate fraction, or `null` when there is nothing honest to divide by.
 *  Clamped, so a malformed frame paints an empty bar rather than one running
 *  past its own track. */
function downloadFraction(downloaded: number, total: number): number | null {
  if (!(total > 0) || !Number.isFinite(downloaded)) return null;
  return Math.min(1, Math.max(0, downloaded / total));
}

/** How a download this document watched ended, or `null` while none has.
 *
 *  Its OWN toast, like the Expose outcome: the download is told only in the
 *  menu while it runs, and this is the result the user is still owed. Memory
 *  was quietly off for the whole download, so its end is news.
 *
 *  `ready` is the resting state of every warm-cache workspace, so the caller
 *  asks only after watching this document's own download. Otherwise every boot
 *  would announce a model that was never not ready. */
export function embeddingModelOutcome(
  state: EmbeddingModelStatus['load_state'] | undefined,
): { title?: string; message: string; tone: 'success' | 'warning' | 'error' } | null {
  switch (state?.kind) {
    case 'failed':
      return { title: 'Embedding model unavailable', message: state.message, tone: 'error' };
    case 'waiting':
      return {
        title: 'Waiting to download the embedding model',
        message:
          'The download has not succeeded yet, so memory search and extraction are off. ' +
          'Lucidos keeps retrying in the background.',
        tone: 'warning',
      };
    case 'ready':
      return {
        message: 'Embedding model ready. Everything you create from now on is searchable in memory.',
        tone: 'success',
      };
    default:
      return null;
  }
}

/** How a finished Expose run reads, or `null` when there is nothing to report.
 *
 *  Its OWN toast, not the run's narration: the narration belongs to the run in
 *  flight, and a second run replaces this result rather than stacking it.
 *
 *  A cancel deliberately produces nothing: the user asked for the run to stop,
 *  and telling them it stopped is noise. */
export function tailscaleServeOutcome(
  run: TailscaleServeProgress | null,
): { message: string; tone: 'success' | 'error' } | null {
  switch (run?.phase) {
    case 'done':
      return {
        message: `Lucidos is now reachable over Tailscale at ${run.url}`,
        tone: 'success',
      };
    case 'failed':
      // Shown verbatim, with no prefix of its own: every message Rust returns
      // already names what failed, and re-framing them here is what once
      // stuttered "Tailscale serve failed: tailscale serve failed: ...".
      return { message: run.message, tone: 'error' };
    default:
      return null;
  }
}
