import { describe, it, expect } from 'vitest';
import {
  backgroundActivities,
  activityToastContent,
  tailscaleServeOutcome,
  MEMORY_NOT_INDEXED_NOTE,
} from './backgroundActivity';
import { parseToastMessage } from '../components/shared/toastMessage';
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

  /** A co-located peer's build spins the badge but reports no elapsed of its
   *  own. Timing it from when THIS client first noticed would be a made-up
   *  number, so it shows none. */
  it('shows no timer for a build whose clock is not ours', () => {
    const peer = { elapsedMs: null, anchoredAt: 1_000_000, pendingCommits: null };
    const [activity] = backgroundActivities(true, null, null, peer, 1_030_000);
    expect(activity.detail).toBeUndefined();
  });

  /** The point of grouping: the reader learns WHAT they are getting before they
   *  read a single sentence. A flat list of subjects was what shipped first, and
   *  over several Applies it read as five `Merge branch 'main' into …` lines. */
  it('describes the build by group, newest first inside each', () => {
    const detail = {
      elapsedMs: 0,
      anchoredAt: 0,
      pendingCommits: {
        total: 3,
        groups: [
          { kind: 'new' as const, total: 1, descriptions: ['memory: one cache per user'] },
          {
            kind: 'fixed' as const,
            total: 2,
            descriptions: ['splash: the brand leaves first', 'todo: a canceled subscription settles'],
          },
        ],
      },
    };
    const [activity] = backgroundActivities(true, null, null, detail, 0);
    expect(activity.note).toBe(
      '3 commits since your running version\n\n' +
        'New\n• memory: one cache per user\n\n' +
        'Fixed\n• splash: the brand leaves first\n• todo: a canceled subscription settles',
    );
  });

  it('counts the commits each group does not name', () => {
    const detail = {
      elapsedMs: 0,
      anchoredAt: 0,
      pendingCommits: {
        total: 9,
        groups: [{ kind: 'fixed' as const, total: 9, descriptions: ['a', 'b', 'c', 'd', 'e'] }],
      },
    };
    const [activity] = backgroundActivities(true, null, null, detail, 0);
    expect(activity.note).toContain('9 commits since your running version');
    expect(activity.note).toContain('• and 4 more');
  });

  /** Counted, never listed, and named for what it is: the number still
   *  reconciles with the heading without forty doc commits crowding out the
   *  work the user is waiting for. */
  it('counts housekeeping on one line instead of listing it', () => {
    const detail = {
      elapsedMs: 0,
      anchoredAt: 0,
      pendingCommits: {
        total: 43,
        groups: [
          { kind: 'new' as const, total: 1, descriptions: ['memory: one cache per user'] },
          { kind: 'housekeeping' as const, total: 42, descriptions: [] },
        ],
      },
    };
    expect(backgroundActivities(true, null, null, detail, 0)[0].note).toBe(
      '43 commits since your running version\n\n' +
        'New\n• memory: one cache per user\n\n' +
        '• 42 housekeeping commits (docs, tests, chores)',
    );
  });

  it('says "1 commit", not "1 commits"', () => {
    const detail = {
      elapsedMs: 0,
      anchoredAt: 0,
      pendingCommits: {
        total: 1,
        groups: [{ kind: 'fixed' as const, total: 1, descriptions: ['the only one'] }],
      },
    };
    expect(backgroundActivities(true, null, null, detail, 0)[0].note).toBe(
      '1 commit since your running version\n\nFixed\n• the only one',
    );
  });

  /** `null` is "git could not answer" and must never be spoken as "0 commits":
   *  that would tell the user nothing is coming while a build is running. A real
   *  zero is silent too, since a spinning badge saying "0 commits" reads as a
   *  contradiction rather than as information. */
  it('says nothing about commits it does not know, and nothing about none', () => {
    const unknown = { elapsedMs: 1000, anchoredAt: 0, pendingCommits: null };
    expect(backgroundActivities(true, null, null, unknown, 0)[0].note).toBeUndefined();
    const zero = { elapsedMs: 1000, anchoredAt: 0, pendingCommits: { total: 0, groups: [] } };
    expect(backgroundActivities(true, null, null, zero, 0)[0].note).toBeUndefined();
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

describe('activityToastContent', () => {
  it('says nothing when there is nothing to report', () => {
    expect(activityToastContent(false, null)).toBeNull();
  });

  /** The caveat the user asked for. Literally true: an embed before the model
   *  lands fails, and the item is dropped rather than stored unindexed. */
  it('warns that work is not indexed while the download runs', () => {
    const content = activityToastContent(false, downloading);
    expect(content?.message).toContain('Downloading embedding model');
    expect(content?.message).toContain(MEMORY_NOT_INDEXED_NOTE);
    expect(content?.progress).toBeCloseTo(0.5);
    expect(content?.settled).toBe(false);
  });

  it('does not carry the caveat when only a build is running', () => {
    const content = activityToastContent(true, null);
    expect(content?.message).toBe('Building new version');
    expect(content?.message).not.toContain(MEMORY_NOT_INDEXED_NOTE);
    expect(content?.progress).toBeNull();
  });

  /** The whole point of the tap: what the toast says has to exceed what the
   *  tooltip already said, and it has to arrive in the shape the toast renders
   *  (heading, then a titled section of bullets, per `shared/toastMessage.ts`). */
  it('says more than the badge tooltip, in the shape the toast renders', () => {
    const content = activityToastContent(true, null, null, false, {
      elapsedMs: 134_000,
      anchoredAt: 500,
      pendingCommits: {
        total: 3,
        groups: [
          { kind: 'fixed', total: 1, descriptions: ['one'] },
          { kind: 'housekeeping', total: 2, descriptions: [] },
        ],
      },
    }, 500);
    expect(content?.message).not.toBe('Building new version');
    const parsed = parseToastMessage(content?.message ?? '');
    expect(parsed.heading).toBe('Building new version, 2m 14s');
    // The count, the one described group, and the counted-only footnote: each
    // its own section, so a group heading can never render as a bullet of the
    // group above it.
    expect(parsed.sections).toHaveLength(3);
    expect(parsed.sections[0].title).toBe('3 commits since your running version');
    expect(parsed.sections[1].title).toBe('Fixed');
    expect(parsed.sections[1].bullets).toEqual(['one']);
    expect(parsed.sections[2].title).toBeUndefined();
    expect(parsed.sections[2].bullets).toEqual(['2 housekeeping commits (docs, tests, chores)']);
  });

  /** Two activities that both have something to add: each note has to land in
   *  its OWN section. Run together, the download's caveat parses as one more
   *  bullet in the build's commit list, i.e. the toast claims the user is
   *  waiting on a commit called "You can keep working…". */
  it('keeps a second note out of the first one\'s bullet list', () => {
    const content = activityToastContent(true, downloading, null, false, {
      elapsedMs: 5_000,
      anchoredAt: 0,
      pendingCommits: {
        total: 2,
        groups: [{ kind: 'fixed', total: 2, descriptions: ['one', 'two'] }],
      },
    }, 0);
    const parsed = parseToastMessage(content?.message ?? '');
    // Four sections: the two activities listed under the heading, the build's
    // count line, its one group, then the download's caveat. Before the fix
    // there were fewer, because the caveat was absorbed as one more bullet of
    // the commit list.
    expect(parsed.sections).toHaveLength(4);
    expect(parsed.sections[1].title).toBe('2 commits since your running version');
    expect(parsed.sections[2].title).toBe('Fixed');
    expect(parsed.sections[2].bullets).toEqual(['one', 'two']);
    expect(parsed.sections[3].title).toBe(MEMORY_NOT_INDEXED_NOTE);
    // The caveat is emphatically NOT a commit.
    expect(parsed.sections[2].bullets).not.toContain(MEMORY_NOT_INDEXED_NOTE);
  });

  it('lists concurrent activities and shows no bar for either', () => {
    const content = activityToastContent(true, downloading);
    expect(content?.message).toContain('• Building new version');
    expect(content?.message).toContain('• Downloading embedding model');
    // Two operations cannot honestly share one track.
    expect(content?.progress).toBeNull();
    // The caveat still applies, because the download is one of them.
    expect(content?.message).toContain(MEMORY_NOT_INDEXED_NOTE);
  });

  /** Wider than `backgroundActivities` on purpose: a toast already on screen
   *  must resolve rather than freeze on its last download frame. */
  it('resolves an open toast when the model lands', () => {
    const content = activityToastContent(false, model({ kind: 'ready' }), null, true);
    expect(content?.settled).toBe(true);
    expect(content?.message).toContain('ready');
    expect(content?.progress).toBe(1);
  });

  it('resolves an open toast when the download stalls', () => {
    const content = activityToastContent(false, model({ kind: 'waiting', attempt: 3 }), null, true);
    expect(content?.settled).toBe(true);
    expect(content?.message).toContain('Waiting to download');
  });

  it('carries the reason when the loader gives up', () => {
    const content = activityToastContent(
      false,
      model({ kind: 'failed', message: 'vector(768) does not fit vector(384)' }),
      null,
      true,
    );
    expect(content?.settled).toBe(true);
    expect(content?.message).toContain('vector(768) does not fit vector(384)');
  });

  /** The reported bug. Every state below is terminal for a model this document
   *  never saw come down, which is the resting state of every warm-cache
   *  workspace: a toast opened to watch a rebuild must clear when the rebuild
   *  ends, not announce an embedding model nobody was waiting on. */
  it('says nothing about a model this document never watched download', () => {
    for (const state of [
      { kind: 'ready' },
      { kind: 'waiting', attempt: 3 },
      { kind: 'failed', message: 'corrupt' },
    ] as const) {
      expect(activityToastContent(false, model(state)), state.kind).toBeNull();
    }
  });

  /** `loading` is not worth a toast of its own, but it must not resolve one
   *  either: the work is still going, it just has no percentage. */
  it('says nothing at all while the ONNX session builds', () => {
    expect(activityToastContent(false, model({ kind: 'loading' }), null, true)).toBeNull();
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
    const content = activityToastContent(false, null, {
      phase: 'awaiting-tailnet-approval',
      url: APPROVAL_URL,
    });
    expect(content?.message).toContain('Waiting for you to enable Serve');
    expect(content?.action).toEqual({
      kind: 'open-url',
      label: 'Enable in Tailscale',
      url: APPROVAL_URL,
    });
    // And a way out of a wait that can legitimately last minutes.
    expect(content?.secondaryAction?.kind).toBe('cancel-tailscale-serve');
    expect(content?.settled).toBe(false);
  });

  /** Actions are deliberately NOT gated on there being a single activity, the
   *  way the progress bar is: an approval the user has to click cannot be
   *  withheld because a model download happens to be running beside it. */
  it('keeps the approval link when another activity is running too', () => {
    const content = activityToastContent(false, downloading, {
      phase: 'awaiting-tailnet-approval',
      url: APPROVAL_URL,
    });
    expect(content?.message).toContain('• Downloading embedding model');
    expect(content?.message).toContain('• Waiting for you to enable Serve');
    expect(content?.progress).toBeNull();
    expect(content?.action?.kind).toBe('open-url');
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

  /** The bug this split exists for. The shared toast only reaches its terminal
   *  branch when NOTHING is in flight, so an outcome routed through it was
   *  dropped outright whenever another activity was running: on a first-run
   *  workspace, an Expose failure during the model download reached nobody. The
   *  outcome now has its own surface, and the shared one keeps narrating the
   *  download. */
  it('does not let a finished run compete with work still in flight', () => {
    const failed = { phase: 'failed', message: 'no MagicDNS name' } as const;
    // The shared toast stays with the download...
    const content = activityToastContent(false, downloading, failed);
    expect(content?.message).toContain('Downloading embedding model');
    expect(content?.settled).toBe(false);
    // ...and the outcome is still reported, independently of it.
    expect(tailscaleServeOutcome(failed)?.message).toBe('no MagicDNS name');
  });

  /** A finished run must not shadow a genuine embedding-model outcome, and the
   *  sibling states keep their own honest tone. */
  it('leaves the embedding model outcomes alone', () => {
    expect(
      activityToastContent(false, model({ kind: 'ready' }), { phase: 'cancelled' }, true)?.tone,
    ).toBe('success');
    expect(
      activityToastContent(false, model({ kind: 'failed', message: 'corrupt' }), null, true)?.tone,
    ).toBe('error');
    expect(activityToastContent(false, model({ kind: 'waiting', attempt: 3 }), null, true)?.tone)
      .toBe('warning');
  });
});
