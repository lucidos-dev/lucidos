import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  syncEmbeddingModelOutcome,
  applyEmbeddingModelStatus,
  loadEmbeddingModelStatus,
  resetBackgroundActivityForTest,
  beginTailscaleServeRun,
  clearTailscaleServeRun,
  applyTailscaleServeProgress,
  EMBEDDING_OUTCOME_TOAST_KEY,
  SERVE_OUTCOME_TOAST_KEY,
  SERVE_RUN_TOAST_KEY,
} from './backgroundActivity';
import {
  toasts,
  dismissToast,
  engineBuilding,
  engineBuildDetail,
  embeddingModelStatus,
  engineRestarting,
  tailscaleServeRun,
} from '../store';
import type { EmbeddingModelLoadState } from '../../api/types';

// Hoisted by vitest, so the module under test sees the stub even though this
// sits below the imports.
const mockGetStatus = vi.fn();
vi.mock('../../api/client', () => ({
  getEmbeddingModelStatus: () => mockGetStatus(),
}));

function setModel(load_state: EmbeddingModelLoadState): void {
  embeddingModelStatus.value = { model_id: 'multilingual-e5-small', load_state };
}

function modelOutcomeToast() {
  return toasts.value.find((t) => t.key === EMBEDDING_OUTCOME_TOAST_KEY);
}

function serveToast() {
  return toasts.value.find((t) => t.key === SERVE_RUN_TOAST_KEY);
}

function serveOutcomeToast() {
  return toasts.value.find((t) => t.key === SERVE_OUTCOME_TOAST_KEY);
}

function downloadFrame(downloaded: number): EmbeddingModelLoadState {
  return { kind: 'downloading', downloaded_bytes: downloaded, total_bytes: 1000 };
}

function reset(): void {
  toasts.value = [];
  engineBuilding.value = false;
  engineBuildDetail.value = null;
  engineRestarting.value = false;
  embeddingModelStatus.value = null;
  tailscaleServeRun.value = null;
  resetBackgroundActivityForTest();
}

describe('the embedding-model download', () => {
  beforeEach(reset);

  /** Progress lives in the Lucidos menu. A download starting raises no toast. */
  it('raises no toast while it runs', () => {
    setModel(downloadFrame(100));
    syncEmbeddingModelOutcome();
    setModel(downloadFrame(600));
    syncEmbeddingModelOutcome();
    expect(toasts.value).toEqual([]);
  });

  it('reports a watched download landing, once', () => {
    setModel(downloadFrame(100));
    syncEmbeddingModelOutcome();
    setModel({ kind: 'ready' });
    syncEmbeddingModelOutcome();
    expect(modelOutcomeToast()?.message).toContain('ready');
    expect(modelOutcomeToast()?.type).toBe('success');

    dismissToast(EMBEDDING_OUTCOME_TOAST_KEY);
    syncEmbeddingModelOutcome();
    expect(modelOutcomeToast()).toBeUndefined();
  });

  it('reports a watched download failing', () => {
    setModel(downloadFrame(430));
    syncEmbeddingModelOutcome();
    setModel({ kind: 'failed', message: 'the cache is corrupt' });
    syncEmbeddingModelOutcome();
    expect(modelOutcomeToast()?.message).toContain('the cache is corrupt');
    expect(modelOutcomeToast()?.type).toBe('error');
  });

  /** A retry that downloads again and then lands is a new result. */
  it('reports again after a stalled download retries and lands', () => {
    setModel(downloadFrame(100));
    syncEmbeddingModelOutcome();
    setModel({ kind: 'waiting', attempt: 3 });
    syncEmbeddingModelOutcome();
    expect(modelOutcomeToast()?.type).toBe('warning');

    setModel(downloadFrame(200));
    syncEmbeddingModelOutcome();
    setModel({ kind: 'ready' });
    syncEmbeddingModelOutcome();
    expect(modelOutcomeToast()?.type).toBe('success');
  });

  /** An existing workspace loads from a warm cache and never downloads. Its
   *  `ready` is not news, so it stays completely silent. */
  it('says nothing about a warm-cache boot', () => {
    setModel({ kind: 'loading' });
    syncEmbeddingModelOutcome();
    setModel({ kind: 'ready' });
    syncEmbeddingModelOutcome();
    expect(toasts.value).toEqual([]);
  });

  /** `showToast` drops everything while the workspace is unavailable. An
   *  outcome it swallowed is still owed, so the next frame reports it. */
  it('is not spent on a report the unavailable workspace swallowed', () => {
    setModel(downloadFrame(100));
    syncEmbeddingModelOutcome();
    engineRestarting.value = true;
    setModel({ kind: 'ready' });
    syncEmbeddingModelOutcome();
    expect(modelOutcomeToast()).toBeUndefined();

    engineRestarting.value = false;
    syncEmbeddingModelOutcome();
    expect(modelOutcomeToast()?.type).toBe('success');
  });

  it('raises nothing for an engine rebuild', () => {
    engineBuilding.value = true;
    setModel({ kind: 'ready' });
    syncEmbeddingModelOutcome();
    expect(toasts.value).toEqual([]);
  });
});

describe('snapshot vs live-frame freshness', () => {
  beforeEach(() => {
    reset();
    mockGetStatus.mockReset();
  });

  /** The regression this counter exists for. The snapshot read and the SSE
   *  stream race, and the loader emits its terminal `ready` frame and then
   *  RETURNS, so there is no later frame to undo a stale write: a `downloading`
   *  body resolving after `ready` would spin the badge for the rest of the
   *  session. */
  it('discards a snapshot that a live frame overtook', async () => {
    let resolveRead: (v: unknown) => void = () => {};
    mockGetStatus.mockImplementation(
      () => new Promise((resolve) => { resolveRead = resolve; }),
    );

    const inFlight = loadEmbeddingModelStatus();

    // The download finishes over SSE while the HTTP read is still open.
    applyEmbeddingModelStatus({
      model_id: 'multilingual-e5-small',
      load_state: { kind: 'ready' },
    });

    // ...and only THEN does the stale body land.
    resolveRead({
      model_id: 'multilingual-e5-small',
      load_state: { kind: 'downloading', downloaded_bytes: 400, total_bytes: 1000 },
    });
    await inFlight;

    expect(embeddingModelStatus.value?.load_state).toEqual({ kind: 'ready' });
  });

  /** A client that connected mid-download reads the snapshot, which is what
   *  lets it report the download's result later. */
  it('applies a snapshot that nothing overtook, and watches the download it names', async () => {
    mockGetStatus.mockResolvedValue({
      model_id: 'multilingual-e5-small',
      load_state: { kind: 'downloading', downloaded_bytes: 400, total_bytes: 1000 },
    });

    await loadEmbeddingModelStatus();

    expect(embeddingModelStatus.value?.load_state).toEqual({
      kind: 'downloading',
      downloaded_bytes: 400,
      total_bytes: 1000,
    });
    applyEmbeddingModelStatus({ model_id: 'multilingual-e5-small', load_state: { kind: 'ready' } });
    expect(modelOutcomeToast()?.type).toBe('success');
  });

  /** A failed read must not disturb the state a live frame already established:
   *  it is an unsolicited probe, and SSE remains the newer truth. */
  it('leaves the live state alone when the read fails', async () => {
    applyEmbeddingModelStatus({
      model_id: 'multilingual-e5-small',
      load_state: { kind: 'ready' },
    });
    mockGetStatus.mockRejectedValue(new Error('offline'));

    await loadEmbeddingModelStatus();

    expect(embeddingModelStatus.value?.load_state).toEqual({ kind: 'ready' });
  });
});

describe('the Expose run keeps its toast', () => {
  const APPROVAL_URL = 'https://login.tailscale.com/f/serve?node=nodeidEXAMPLE1234';

  beforeEach(reset);

  /** A button the user just pressed, so it narrates every time. Pressing Expose
   *  twice in a session must not go silent the second time. */
  it('opens its toast on every run', () => {
    beginTailscaleServeRun();
    expect(serveToast()?.message).toContain('Setting up Tailscale access');

    applyTailscaleServeProgress({ phase: 'done', url: 'https://mymac.tailnet-name.ts.net' });
    beginTailscaleServeRun();
    expect(serveToast()?.message).toContain('Setting up Tailscale access');
  });

  it('narrates each step in place, without stacking toasts', () => {
    beginTailscaleServeRun();
    const firstId = serveToast()?.id;

    applyTailscaleServeProgress({ phase: 'configuring' });
    expect(serveToast()?.message).toContain('Configuring tailscale serve');

    // A step with a note shows its label as the title, over the note.
    applyTailscaleServeProgress({ phase: 'waiting-for-https' });
    expect(serveToast()?.title).toContain('Waiting for HTTPS');
    expect(serveToast()?.message).toContain('first certificate');

    expect(toasts.value.filter((t) => t.key === SERVE_RUN_TOAST_KEY)).toHaveLength(1);
    expect(serveToast()?.id).toBe(firstId);
    // Indeterminate throughout: a spinner, never a fabricated bar.
    expect(serveToast()?.spinning).toBe(true);
    expect(serveToast()?.progress ?? null).toBeNull();
  });

  /** The step that blocks on the user: the link the CLI printed, offered as a
   *  button, with a way out of the wait. */
  it('turns the tailnet-approval step into a real button', () => {
    beginTailscaleServeRun();
    applyTailscaleServeProgress({ phase: 'awaiting-tailnet-approval', url: APPROVAL_URL });
    expect(serveToast()?.action?.label).toBe('Enable in Tailscale');
    expect(serveToast()?.secondaryAction?.label).toBe('Cancel');
    expect(serveToast()?.message).toContain('Open the link, approve it');
  });

  /** The run is the only background work that narrates in a toast. A download
   *  running beside it stays in the menu. */
  it('narrates only itself', () => {
    setModel(downloadFrame(100));
    syncEmbeddingModelOutcome();
    beginTailscaleServeRun();
    expect(serveToast()?.message).not.toContain('Downloading');
    expect(toasts.value).toHaveLength(1);
  });

  it('reports the address on success, and lets the badge stop', () => {
    beginTailscaleServeRun();
    applyTailscaleServeProgress({ phase: 'done', url: 'https://mymac.tailnet-name.ts.net' });
    expect(serveOutcomeToast()?.message).toContain('https://mymac.tailnet-name.ts.net');
    expect(serveOutcomeToast()?.type).toBe('success');
    expect(tailscaleServeRun.value).toBeNull();
    expect(serveToast()).toBeUndefined();
  });

  /** A failure reaches the user even if they had closed the narration, because
   *  it is the outcome of something they pressed. */
  it('reports a failure even after the narration was closed', () => {
    beginTailscaleServeRun();
    dismissToast(SERVE_RUN_TOAST_KEY);

    applyTailscaleServeProgress({ phase: 'failed', message: 'no MagicDNS name' });
    expect(serveOutcomeToast()?.message).toBe('no MagicDNS name');
    expect(serveOutcomeToast()?.type).toBe('error');
    expect(tailscaleServeRun.value).toBeNull();
  });

  it('clears everything on a cancel, with nothing to read', () => {
    beginTailscaleServeRun();
    applyTailscaleServeProgress({ phase: 'cancelled' });
    expect(serveToast()).toBeUndefined();
    expect(serveOutcomeToast()).toBeUndefined();
    expect(tailscaleServeRun.value).toBeNull();
  });

  /** `showToast` with a key creates a missing toast, so a closed narration must
   *  not come back on the next frame. The run's menu row is how the user sees
   *  it again. */
  it('stays dismissed for mid-run frames after the user closes it', () => {
    beginTailscaleServeRun();
    dismissToast(SERVE_RUN_TOAST_KEY);

    applyTailscaleServeProgress({ phase: 'configuring' });
    applyTailscaleServeProgress({ phase: 'awaiting-tailnet-approval', url: APPROVAL_URL });
    expect(serveToast()).toBeUndefined();
    expect(tailscaleServeRun.value?.phase).toBe('awaiting-tailnet-approval');
  });

  /** For what Rust could not report at all: a rejected invoke, an ACL denial, a
   *  dead bridge. The page shows the error; this only has to stop the badge. */
  it('clears the run without narrating when no frame ever arrived', () => {
    beginTailscaleServeRun();
    clearTailscaleServeRun();
    expect(tailscaleServeRun.value).toBeNull();
    expect(serveToast()).toBeUndefined();
  });
});
