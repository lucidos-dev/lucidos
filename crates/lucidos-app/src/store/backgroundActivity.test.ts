import { describe, it, expect } from 'vitest';
import {
  backgroundActivities,
  embeddingModelOutcome,
  tailscaleServeActivity,
  tailscaleServeOutcome,
  MEMORY_NOT_INDEXED_NOTE,
} from './backgroundActivity';
import type { EmbeddingModelStatus, EmbeddingModelLoadState } from '../api/types';

function model(load_state: EmbeddingModelLoadState): EmbeddingModelStatus {
  return { model_id: 'multilingual-e5-small', load_state };
}

const downloading = model({
  kind: 'downloading',
  downloaded_bytes: 244_000_000,
  total_bytes: 488_000_000,
});

describe('backgroundActivities', () => {
  it('reports nothing when nothing is happening', () => {
    expect(backgroundActivities(false, null)).toEqual([]);
    expect(backgroundActivities(false, model({ kind: 'ready' }))).toEqual([]);
  });

  it('reports a dev engine rebuild, with no fabricated percentage', () => {
    const [activity, ...rest] = backgroundActivities(true, null);
    expect(rest).toEqual([]);
    expect(activity.kind).toBe('engine-build');
    expect(activity.label).toBe('Building new version');
    // A cargo build reports no progress; inventing a bar would be worse than
    // the spinner the toast falls back to.
    expect(activity.progress).toBeNull();
  });

  /** The counter advances from the CLIENT anchor, so it moves between the ~4s
   *  polls (nothing pushes a per-second frame for a build) without ever
   *  differencing the engine's clock against the browser's. */
  it('counts the build up from the client anchor, not the engine clock', () => {
    const detail = { elapsedMs: 8_000, anchoredAt: 1_000_000, pendingCommits: null };
    expect(backgroundActivities(true, null, null, detail, 1_000_000)[0].detail).toBe('8s');
    // Six seconds later, with no new poll, the same detail reads six higher.
    expect(backgroundActivities(true, null, null, detail, 1_006_000)[0].detail).toBe('14s');
    expect(backgroundActivities(true, null, null, detail, 1_112_000)[0].detail).toBe('2m 0s');
  });

  /** A build waiting for a build slot is not compiling, so it must not say it
   *  is. The timer and the commits carry on unchanged: they describe the Apply,
   *  not the compile. */
  it('names a queued build as queued, keeping its timer and commits', () => {
    const commits = {
      total: 1,
      groups: [{ kind: 'fixed' as const, total: 1, descriptions: ['a fix'] }],
    };
    const queued = {
      elapsedMs: 8_000,
      anchoredAt: 1_000_000,
      pendingCommits: commits,
      queuedBehind: ['make lint', 'engine tests', 'engine tests'],
    };
    const [activity] = backgroundActivities(true, null, null, queued, 1_000_000);
    expect(activity.label).toBe('New version queued');
    expect(activity.detail).toBe('8s');
    expect(activity.queued).toBe(
      'All 3 build slots are busy: make lint, engine tests, engine tests. ' +
        'It takes the next one that frees up.',
    );
    // The commits stay with the build detail, which the menu lays out.
    expect(activity.note).toBe('You can switch to it once the build finishes.');

    const compiling = { ...queued, queuedBehind: undefined };
    const [running] = backgroundActivities(true, null, null, compiling, 1_000_000);
    expect(running.label).toBe('Building new version');
    expect(running.queued).toBeUndefined();
  });

  it('says one slot, not "1 slots"', () => {
    const queued = { elapsedMs: 0, anchoredAt: 0, pendingCommits: null, queuedBehind: ['make lint'] };
    const [activity] = backgroundActivities(true, null, null, queued, 0);
    expect(activity.queued).toBe('The one build slot is busy: make lint. It takes it as soon as it frees up.');
  });

  it('reports the frontend rebuild after a frontend-only Apply, with a live timer', () => {
    const refresh = { elapsedMs: 4_000, anchoredAt: 1_000_000 };
    const [activity, ...rest] = backgroundActivities(false, null, null, null, 1_003_000, refresh);
    expect(rest).toEqual([]);
    expect(activity.kind).toBe('frontend-refresh');
    expect(activity.label).toBe('Building frontend');
    expect(activity.progress).toBeNull();
    expect(activity.detail).toBe('7s');
    expect(backgroundActivities(false, null, null, null, 1_003_000, null)).toEqual([]);
  });

  it('lists the frontend rebuild after the engine build when both run', () => {
    const refresh = { elapsedMs: 0, anchoredAt: 0 };
    const kinds = backgroundActivities(true, null, null, null, 0, refresh).map((a) => a.kind);
    expect(kinds).toEqual(['engine-build', 'frontend-refresh']);
  });

  /** A co-located peer's build spins the badge but reports no elapsed of its
   *  own. Timing it from when THIS client first noticed would be a made-up
   *  number, so it shows none. */
  it('shows no timer for a build whose clock is not ours', () => {
    const peer = { elapsedMs: null, anchoredAt: 1_000_000, pendingCommits: null };
    const [activity] = backgroundActivities(true, null, null, peer, 1_030_000);
    expect(activity.detail).toBeUndefined();
  });

  /** What a build brings is laid out by the menu detail from the engine's
   *  groups. The note is what the detail says when git cannot count them. */
  it('says what follows each build', () => {
    expect(backgroundActivities(true, null)[0].note).toBe('You can switch to it once the build finishes.');
    const refresh = { elapsedMs: 1_000, anchoredAt: 0 };
    expect(backgroundActivities(false, null, null, null, 0, refresh)[0].note)
      .toBe('A Refresh prompt appears once the build finishes.');
  });

  it('reports a download with byte detail and a determinate fraction', () => {
    const [activity] = backgroundActivities(false, downloading);
    expect(activity.kind).toBe('embedding-model');
    expect(activity.label).toBe('Downloading embedding model');
    expect(activity.detail).toContain('of');
    expect(activity.progress).toBeCloseTo(0.5);
  });

  /** The badge exists to show work in flight. Building the ONNX session takes a
   *  few seconds on EVERY boot, warm cache included, so counting it would flash
   *  a spinner every time the app opens. */
  it('does not count the post-download ONNX load', () => {
    expect(backgroundActivities(false, model({ kind: 'loading' }))).toEqual([]);
  });

  /** Neither is work in progress, and both already have a notification. An
   *  offline machine would otherwise spin the badge forever. */
  it('does not count a stalled or abandoned load', () => {
    expect(backgroundActivities(false, model({ kind: 'waiting', attempt: 4 }))).toEqual([]);
    expect(backgroundActivities(false, model({ kind: 'failed', message: 'bad' }))).toEqual([]);
  });

  it('reports both activities at once, build first', () => {
    const activities = backgroundActivities(true, downloading);
    expect(activities.map((a) => a.kind)).toEqual(['engine-build', 'embedding-model']);
  });

  it('withholds the fraction when no total is known yet', () => {
    const [activity] = backgroundActivities(
      false,
      model({ kind: 'downloading', downloaded_bytes: 1024, total_bytes: 0 }),
    );
    // A frame can arrive before any file has declared its size. Show the bytes
    // so far and no bar, rather than a bar built on a zero denominator.
    expect(activity.progress).toBeNull();
    expect(activity.detail).toBeTruthy();
  });

  it('clamps a nonsensical frame instead of overrunning the track', () => {
    const [activity] = backgroundActivities(
      false,
      model({ kind: 'downloading', downloaded_bytes: 900, total_bytes: 100 }),
    );
    expect(activity.progress).toBe(1);
  });
});

describe('embeddingModelOutcome', () => {
  it('reads a landed model as a success', () => {
    expect(embeddingModelOutcome({ kind: 'ready' })?.tone).toBe('success');
  });

  it('reads a stalled download as a warning that says it retries', () => {
    const outcome = embeddingModelOutcome({ kind: 'waiting', attempt: 3 });
    expect(outcome?.tone).toBe('warning');
    expect(outcome?.title).toBe('Waiting to download the embedding model');
    expect(outcome?.message).toContain('keeps retrying');
  });

  it('carries the reason when the loader gives up', () => {
    const outcome = embeddingModelOutcome({ kind: 'failed', message: 'vector(768) does not fit vector(384)' });
    expect(outcome?.tone).toBe('error');
    expect(outcome?.title).toBe('Embedding model unavailable');
    expect(outcome?.message).toBe('vector(768) does not fit vector(384)');
  });

  /** Work still in progress has no outcome yet. */
  it('says nothing while the model is still coming', () => {
    expect(embeddingModelOutcome({ kind: 'loading' })).toBeNull();
    expect(embeddingModelOutcome(downloading.load_state)).toBeNull();
    expect(embeddingModelOutcome(undefined)).toBeNull();
  });

  it('keeps the memory caveat on the download itself', () => {
    expect(backgroundActivities(false, downloading)[0].note).toBe(MEMORY_NOT_INDEXED_NOTE);
  });
});

/** The Expose run (`tailscale serve`). The link in these tests is the one the
 *  real CLI printed on 2026-08-02 for a tailnet without Serve enabled. */
const APPROVAL_URL = 'https://login.tailscale.com/f/serve?node=nodeidEXAMPLE1234';

describe('the Expose run on the badge', () => {
  it('spins the badge for every step of a run in flight', () => {
    for (const run of [
      { phase: 'starting' },
      { phase: 'checking-tailnet' },
      { phase: 'configuring' },
      { phase: 'awaiting-tailnet-approval', url: APPROVAL_URL },
      { phase: 'waiting-for-https' },
    ] as const) {
      const activities = backgroundActivities(false, null, run);
      expect(activities, run.phase).toHaveLength(1);
      expect(activities[0].kind).toBe('tailscale-serve');
      expect(activities[0].label, run.phase).not.toBe('');
      // Not one step of this flow can honestly report a fraction.
      expect(activities[0].progress, run.phase).toBeNull();
    }
  });

  /** The badge shows work IN FLIGHT, so a run that is over must stop it. A
   *  spinning badge with nothing behind it is the failure this pins. */
  it('stops spinning the moment the run ends', () => {
    for (const run of [
      { phase: 'done', url: 'https://mymac.tailnet-name.ts.net' },
      { phase: 'failed', message: 'no' },
      { phase: 'cancelled' },
    ] as const) {
      expect(backgroundActivities(false, null, run), run.phase).toEqual([]);
    }
    expect(backgroundActivities(false, null, null)).toEqual([]);
  });

  /** The whole point of the change. The CLI prints this link and then blocks
   *  polling until someone visits it; the old code killed the child at 20s and
   *  threw the link away with the pipes. */
  it('offers the approval link the CLI printed, verbatim', () => {
    const activity = tailscaleServeActivity({ phase: 'awaiting-tailnet-approval', url: APPROVAL_URL });
    expect(activity?.label).toContain('Waiting for you to enable Serve');
    expect(activity?.action).toEqual({
      kind: 'open-url',
      label: 'Enable in Tailscale',
      url: APPROVAL_URL,
    });
    // And a way out of a wait that can legitimately last minutes.
    expect(activity?.secondaryAction?.kind).toBe('cancel-tailscale-serve');
  });

  it('reports the address on success, and reads as a success', () => {
    const outcome = tailscaleServeOutcome({
      phase: 'done',
      url: 'https://mymac.tailnet-name.ts.net',
    });
    expect(outcome?.message).toContain('https://mymac.tailnet-name.ts.net');
    expect(outcome?.tone).toBe('success');
  });

  /** Shown verbatim, with no prefix: every message Rust returns already names
   *  what failed, and re-framing them here is what once produced "Tailscale
   *  serve failed: tailscale serve failed: ...". */
  it('reports a failure verbatim, and reads as a failure', () => {
    const message = 'This Mac is not on a tailnet yet. Sign in to Tailscale first.';
    expect(tailscaleServeOutcome({ phase: 'failed', message })).toEqual({
      message,
      tone: 'error',
    });
  });

  /** A cancel is the user getting what they asked for, so there is nothing to
   *  report. The surface simply clears. */
  it('says nothing at all about a cancelled run', () => {
    expect(tailscaleServeOutcome({ phase: 'cancelled' })).toBeNull();
    expect(tailscaleServeOutcome(null)).toBeNull();
    // Nor about one still in flight.
    expect(tailscaleServeOutcome({ phase: 'configuring' })).toBeNull();
  });
});
