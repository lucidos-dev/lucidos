/**
 * Background activity: long-running work surfaced on the brand badge and, when
 * the user taps it, in a status toast.
 *
 * Three activities today:
 *
 * - a dev engine rebuild (`engineBuilding`, which already drove the spinning
 *   badge);
 * - the embedding-model download, which until now ran completely invisibly: the
 *   ~465 MB first-run fetch takes minutes, during which memory search,
 *   extraction and semantic thread search are dead;
 * - an **Expose** run (`tailscale serve`), which the user DOES start, and which
 *   can legitimately spend minutes waiting for a tailnet approval.
 *
 * That last one stretches "work the user did not start", and belongs here
 * anyway: it is long-running, it outlives the pane that launched it, and the
 * badge is the one place in the app that says something is happening. The
 * difference it does make is in the AUTO-OPEN rule, which lives in
 * `actions/backgroundActivity.ts`: a download nobody asked for opens its toast
 * once per document, while a button press opens it every time.
 *
 * Everything here is a PURE function of explicit arguments, with the signal
 * reads left to the callsite (the `currentWorkspaceRefreshState` precedent in
 * ControlPanel.tsx), so the derivation and the toast copy are unit-testable
 * without a render. Actions are carried as DESCRIPTORS rather than callbacks for
 * the same reason: a closure in here would be the end of that.
 */

import { signal } from '@preact/signals';
import type { EmbeddingModelStatus } from '../api/types';
import type { TailscaleServeProgress } from '../utils/tauri';
import type { PendingCommits } from '../api/client';
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

/** What the toast can say about the dev engine rebuild beyond "one is running",
 *  or `null` when there is nothing to narrate.
 *
 *  The odd one out among the three feeds: the other two are PUSHED a frame per
 *  update (`EmbeddingModelStatusChanged` over SSE, `tailscale-serve-progress`
 *  over Tauri), so re-rendering per frame keeps them current. A build emits only
 *  its transitions, so this arrives on the ~4s version-status poll and the
 *  seconds in between are counted locally. Hence `anchoredAt`. */
export interface EngineBuildDetail {
  /** Build age in ms as the ENGINE measured it, at the moment `anchoredAt` was
   *  taken. `null` when the engine reported none, which is the co-located peer's
   *  build: the badge spins for it, but its clock is not ours to read. */
  elapsedMs: number | null;
  /** The client's own `Date.now()` when `elapsedMs` arrived. The live counter is
   *  `elapsedMs + (now - anchoredAt)`, so it advances between polls without ever
   *  differencing the engine's wall clock against the browser's. Two clocks that
   *  disagree would otherwise show a wrong, possibly negative, build age. */
  anchoredAt: number;
  /** The commits this build will bring, or `null` when git couldn't say. Those
   *  are different answers: `{ total: 0 }` is "nothing pending", `null` is "we
   *  don't know", and only the first may be stated out loud. */
  pendingCommits: PendingCommits | null;
}

/** Latest build narration, or `null` when no rebuild is in flight. Written only
 *  by `setEngineBuilding` (`store/actions/engine-update.ts`), paired with
 *  `engineBuilding`, so a cleared boolean can never leave a stale timer behind.
 *
 *  Nullable rather than `Loadable<T>`, matching the two feeds above. */
export const engineBuildDetail = signal<EngineBuildDetail | null>(null);

/** Something the status toast offers to DO about an activity, as data.
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
  kind: 'engine-build' | 'embedding-model' | 'tailscale-serve';
  /** One line naming what is happening, e.g. "Downloading embedding model". */
  label: string;
  /** Appended after the label, e.g. "212 MB of 465 MB". */
  detail?: string;
  /** Fraction in [0, 1], or `null` when there is no honest percentage. */
  progress: number | null;
  /** A further line the status toast shows under the label. */
  note?: string;
  /** The primary thing to do about it, e.g. approve Serve for the tailnet. */
  action?: ActivityAction;
  /** The way out, e.g. cancelling a run that is waiting on the user. */
  secondaryAction?: ActivityAction;
}

/** What the toast says while the embedding model is still coming down.
 *
 *  Literally true, and the reason it is worth saying: an embed attempted before
 *  the model lands fails, and `index_memory_inner_impl` drops the item rather
 *  than storing it unindexed. The post-install sweep only re-embeds rows that
 *  already exist with a stale model id, so nothing created in this window is
 *  recovered without a manual memory rebuild (docs/known-gaps.md). */
export const MEMORY_NOT_INDEXED_NOTE =
  'You can keep working. Anything created before this finishes will not be searchable in memory.';

/** Background work currently in flight, in the order the toast lists it.
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
 *    spin the badge forever. An OPEN toast still narrates them (see
 *    {@link activityToastContent}) so a download that stalls while the user is
 *    watching says so instead of freezing at 43%.
 *  - `ready` does NOT, obviously. */
export function backgroundActivities(
  engineBuilding: boolean,
  model: EmbeddingModelStatus | null,
  serveRun: TailscaleServeProgress | null = null,
  buildDetail: EngineBuildDetail | null = null,
  nowMs: number = Date.now(),
): BackgroundActivity[] {
  const activities: BackgroundActivity[] = [];
  if (engineBuilding) {
    activities.push({
      kind: 'engine-build',
      label: 'Building new version',
      // A cargo build reports no percentage, and inventing one would be worse
      // than the spinner the toast falls back to. What it CAN say honestly is
      // how long it has been going and what it will bring.
      progress: null,
      detail: buildElapsedDetail(buildDetail, nowMs),
      note: pendingCommitsNote(buildDetail?.pendingCommits ?? null),
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
 *  FLIGHT, so a finished run must stop it spinning. Their narration is picked up
 *  by {@link activityToastContent}, which is deliberately wider. */
export function tailscaleServeActivity(
  run: TailscaleServeProgress | null,
): BackgroundActivity | null {
  if (!run) return null;
  // Every step of this flow is indeterminate, so the toast spins throughout.
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

/** How long the build has been running, as the toast's heading suffix, or
 *  `undefined` when there is no honest number.
 *
 *  Counted from the client anchor rather than the engine's clock, and re-derived
 *  on every call, which is what lets a 1s ticker advance it between the ~4s
 *  polls. A peer's build reports no elapsed at all (`elapsedMs === null`), and
 *  that stays blank instead of quietly timing from when THIS client noticed. */
function buildElapsedDetail(
  detail: EngineBuildDetail | null,
  nowMs: number,
): string | undefined {
  if (detail?.elapsedMs == null) return undefined;
  return formatElapsed(detail.elapsedMs + Math.max(0, nowMs - detail.anchoredAt));
}

/** The commits this build will bring, as a toast section: a title line naming
 *  the count, then one `• ` bullet per subject (see `shared/toastMessage.ts` for
 *  the parse contract this writes to).
 *
 *  `undefined` in the two cases that must not become text. **Unknown** (`null`,
 *  git could not answer) would otherwise be reported as "0 commits", telling the
 *  user nothing is coming while a build runs. **Zero** is a real answer, but not
 *  one worth a section: the badge is spinning, so something IS being built, and
 *  "0 commits since your running version" reads as a contradiction rather than
 *  information (it happens legitimately, e.g. a rebuild of a dirty tree whose
 *  commit has not moved). */
function pendingCommitsNote(commits: PendingCommits | null): string | undefined {
  if (!commits || commits.total === 0 || commits.subjects.length === 0) return undefined;
  const title =
    commits.total === 1
      ? '1 commit since your running version'
      : `${commits.total} commits since your running version`;
  const lines = commits.subjects.map((s) => `• ${s}`);
  const hidden = commits.total - commits.subjects.length;
  if (hidden > 0) lines.push(`• and ${hidden} more`);
  return [title, ...lines].join('\n');
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

export interface ActivityToastContent {
  /** Ready for `showToast`: heading on line 1, further lines below. */
  message: string;
  /** Determinate progress, or `null` for the spinner. */
  progress: number | null;
  /** Nothing is in flight any more, so the caller may auto-dismiss. */
  settled: boolean;
  /** Which `ToastType` this reads as. A failure must not arrive dressed as a
   *  success, which is what a bare `settled ? 'success' : 'info'` did to the
   *  embedding model's own terminal states. */
  tone: 'info' | 'success' | 'warning' | 'error';
  /** The primary offer, e.g. the tailnet approval link. */
  action?: ActivityAction;
  /** The way out, e.g. cancelling the run. */
  secondaryAction?: ActivityAction;
}

/** What the status toast should say right now, or `null` when there is nothing
 *  to report at all (no activity, and nothing terminal worth naming).
 *
 *  Wider than {@link backgroundActivities} on purpose. The badge only shows
 *  work in flight, but a toast the user already has open must keep telling the
 *  truth when that work stalls or finishes, rather than freezing on its last
 *  progress frame.
 *
 *  `downloadSeen` is whether this document ever watched the embedding model
 *  actually DOWNLOADING, and it gates every terminal model outcome below. See
 *  the comment at that branch for why the toast may not speak about work it
 *  never narrated. */
export function activityToastContent(
  engineBuilding: boolean,
  model: EmbeddingModelStatus | null,
  serveRun: TailscaleServeProgress | null = null,
  downloadSeen = false,
  buildDetail: EngineBuildDetail | null = null,
  nowMs: number = Date.now(),
): ActivityToastContent | null {
  const activities = backgroundActivities(engineBuilding, model, serveRun, buildDetail, nowMs);
  if (activities.length > 0) {
    const lines = activities.map((a) => (a.detail ? `${a.label}, ${a.detail}` : a.label));
    const notes = activities.map((a) => a.note).filter((n): n is string => !!n);
    // One activity reads as a sentence; several read as a list, which is what
    // the toast's own bullet syntax renders.
    const heading =
      activities.length === 1 ? lines[0] : ['Working in the background', ...lines.map(bullet)].join('\n');
    // A BLANK LINE BEFORE EACH note, not just before the first: that is what
    // makes each one its own section (`shared/toastMessage.ts` starts a section
    // at a blank line and treats every later non-bullet line in it as another
    // bullet). Concatenating them meant a second note landed inside the first
    // one's bullet list, so the download's "not searchable in memory" caveat
    // rendered as if it were one of the commits the build is bringing. For a
    // single note the two forms are identical.
    const message =
      notes.length > 0 ? [heading, ...notes.flatMap((n) => ['', n])].join('\n') : heading;
    // A determinate bar only makes sense while exactly one activity owns it.
    // Two concurrent operations sharing one track would show neither honestly.
    const progress = activities.length === 1 ? activities[0].progress : null;
    // Actions are NOT gated on there being a single activity, unlike the bar:
    // an approval link the user has to click cannot be withheld just because a
    // model download happens to be running beside it. Only one activity offers
    // any today, so first-wins is unambiguous.
    return {
      message,
      progress,
      settled: false,
      tone: 'info',
      action: activities.find((a) => a.action)?.action,
      secondaryAction: activities.find((a) => a.secondaryAction)?.secondaryAction,
    };
  }

  // Nothing in flight. Report a terminal embedding-model outcome so an open
  // toast resolves instead of lingering on its last download frame, but ONLY
  // when this document actually watched that download.
  //
  // The toast is shared, so without that gate it speaks about work it never
  // narrated. `ready` is the resting state of every warm-cache workspace, which
  // is every workspace after its first boot: open the toast on the spinning
  // badge to watch an engine rebuild, and the moment the rebuild ends the same
  // toast announces "Embedding model ready. Everything you create from now on
  // is searchable in memory.", about a model that was never not ready, in a
  // toast the user opened to read about something else (reported 2026-08-03,
  // right after a Switch to new version). With the gate, a toast that was
  // narrating something else simply clears, which is what its own activity
  // ending means.
  //
  // `waiting` and `failed` are gated with it rather than carved out, for the
  // reason {@link backgroundActivities} already keeps them off the badge: both
  // are announced by the loader's own notifications, so the one thing lost is a
  // problem report hijacking a toast about an unrelated activity. A download
  // this document DID watch still resolves into all three, which is the case
  // the terminal branch was written for.
  //
  // A finished Expose run is deliberately NOT reported here. Its outcome is the
  // result of something the user pressed, so it gets its own toast rather than
  // competing for this one (`tailscaleServeOutcome`, shown by
  // `actions/backgroundActivity.ts`). Routing it through here swallowed it
  // outright whenever another activity was in flight, because this branch is
  // only reached when nothing is: a first-run workspace downloading the
  // embedding model would report no Expose failure at all.
  if (!downloadSeen) return null;
  const state = model?.load_state;
  if (state?.kind === 'failed') {
    return {
      message: `Embedding model unavailable\n\n${state.message}`,
      progress: null,
      settled: true,
      tone: 'error',
    };
  }
  if (state?.kind === 'waiting') {
    return {
      message:
        'Waiting to download the embedding model\n\n' +
        'The download has not succeeded yet, so memory search and extraction are off. ' +
        'Lucidos keeps retrying in the background.',
      progress: null,
      settled: true,
      tone: 'warning',
    };
  }
  if (state?.kind === 'ready') {
    return {
      message: 'Embedding model ready. Everything you create from now on is searchable in memory.',
      progress: 1,
      settled: true,
      tone: 'success',
    };
  }
  return null;
}

/** How a finished Expose run reads, or `null` when there is nothing to report.
 *
 *  Its OWN toast, not the background-activity one: this is the outcome of
 *  something the user pressed, and the shared toast belongs to whatever is in
 *  flight. Sharing it meant the outcome was dropped entirely whenever another
 *  activity was running, since the shared toast only reaches its terminal
 *  branch once nothing is.
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

function bullet(line: string): string {
  return `• ${line}`;
}
