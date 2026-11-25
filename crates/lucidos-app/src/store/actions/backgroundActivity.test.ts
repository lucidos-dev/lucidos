import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
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
  activityAction,
  subscribeToTailscaleServeProgress,
  unsubscribeFromTailscaleServeProgress,
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

const mockGetStatus = vi.fn();
vi.mock('../../api/client', () => ({
  getEmbeddingModelStatus: () => mockGetStatus(),
}));

const mockIsTauri = vi.fn(() => false);
vi.mock('../../utils/platform', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/platform')>()),
  isTauri: () => mockIsTauri(),
}));

const mockOpenExternalUrl = vi.fn();
vi.mock('../../utils/openExternalUrl', () => ({
  openExternalUrl: (url: string) => mockOpenExternalUrl(url),
}));

const mockListen = vi.fn();
const mockOpenExternal = vi.fn();
const mockCancelServe = vi.fn();
vi.mock('../../utils/tauri', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/tauri')>()),
  listen: (...args: unknown[]) => mockListen(...args),
  openExternal: (url: string) => mockOpenExternal(url),
  cancelTailscaleServe: () => mockCancelServe(),
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

  it('says nothing about a warm-cache boot', () => {
    setModel({ kind: 'loading' });
    syncEmbeddingModelOutcome();
    setModel({ kind: 'ready' });
    syncEmbeddingModelOutcome();
    expect(toasts.value).toEqual([]);
  });

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

  it('discards a snapshot that a live frame overtook', async () => {
    let resolveRead: (v: unknown) => void = () => {};
    mockGetStatus.mockImplementation(
      () => new Promise((resolve) => { resolveRead = resolve; }),
    );

    const inFlight = loadEmbeddingModelStatus();

    applyEmbeddingModelStatus({
      model_id: 'multilingual-e5-small',
      load_state: { kind: 'ready' },
    });

    resolveRead({
      model_id: 'multilingual-e5-small',
      load_state: { kind: 'downloading', downloaded_bytes: 400, total_bytes: 1000 },
    });
    await inFlight;

    expect(embeddingModelStatus.value?.load_state).toEqual({ kind: 'ready' });
  });

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

    applyTailscaleServeProgress({ phase: 'waiting-for-https' });
    expect(serveToast()?.title).toContain('Waiting for HTTPS');
    expect(serveToast()?.message).toContain('first certificate');

    expect(toasts.value.filter((t) => t.key === SERVE_RUN_TOAST_KEY)).toHaveLength(1);
    expect(serveToast()?.id).toBe(firstId);
    expect(serveToast()?.spinning).toBe(true);
    expect(serveToast()?.progress ?? null).toBeNull();
  });

  it('turns the tailnet-approval step into a real button', () => {
    beginTailscaleServeRun();
    applyTailscaleServeProgress({ phase: 'awaiting-tailnet-approval', url: APPROVAL_URL });
    expect(serveToast()?.action?.label).toBe('Enable in Tailscale');
    expect(serveToast()?.secondaryAction?.label).toBe('Cancel');
    expect(serveToast()?.message).toContain('Open the link, approve it');
  });

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

  it('stays dismissed for mid-run frames after the user closes it', () => {
    beginTailscaleServeRun();
    dismissToast(SERVE_RUN_TOAST_KEY);

    applyTailscaleServeProgress({ phase: 'configuring' });
    applyTailscaleServeProgress({ phase: 'awaiting-tailnet-approval', url: APPROVAL_URL });
    expect(serveToast()).toBeUndefined();
    expect(tailscaleServeRun.value?.phase).toBe('awaiting-tailnet-approval');
  });

  it('clears the run without narrating when no frame ever arrived', () => {
    beginTailscaleServeRun();
    clearTailscaleServeRun();
    expect(tailscaleServeRun.value).toBeNull();
    expect(serveToast()).toBeUndefined();
  });
});

describe('the activity link opens the way the platform needs', () => {
  beforeEach(() => {
    reset();
    mockIsTauri.mockReturnValue(false);
    mockOpenExternalUrl.mockReset();
    mockOpenExternal.mockReset();
  });

  it('opens a link through the browser when not in the desktop app', () => {
    activityAction({ kind: 'open-url', label: 'Open', url: 'https://example.com/x' })?.onClick();
    expect(mockOpenExternalUrl).toHaveBeenCalledWith('https://example.com/x');
    expect(mockOpenExternal).not.toHaveBeenCalled();
  });
});

describe('the Expose run cancel and progress subscription', () => {
  beforeEach(() => {
    reset();
    mockIsTauri.mockReturnValue(true);
    mockListen.mockReset();
    mockCancelServe.mockReset();
  });

  afterEach(() => {
    unsubscribeFromTailscaleServeProgress();
    mockIsTauri.mockReturnValue(false);
  });

  it('a rejected cancel says so in an error toast and leaves the run set', async () => {
    mockCancelServe.mockRejectedValue(new Error('bridge gone'));
    tailscaleServeRun.value = { phase: 'starting' };

    activityAction({ kind: 'cancel-tailscale-serve', label: 'Cancel' })?.onClick();
    await new Promise((r) => setTimeout(r, 0));

    const toast = toasts.value.find((t) => t.message.includes("Couldn't cancel"));
    expect(toast?.type).toBe('error');
    expect(tailscaleServeRun.value).toEqual({ phase: 'starting' });
  });

  it('a second subscribe while the first listen is pending registers no second listener', async () => {
    let resolveListen: (unlisten: () => void) => void = () => {};
    mockListen.mockImplementation(() => new Promise((resolve) => { resolveListen = resolve; }));

    const first = subscribeToTailscaleServeProgress();
    const second = subscribeToTailscaleServeProgress();
    resolveListen(() => {});
    await Promise.all([first, second]);

    expect(mockListen).toHaveBeenCalledTimes(1);
  });

  it('a failed progress subscription does not throw, and the next mount retries it', async () => {
    mockListen
      .mockRejectedValueOnce(new Error('bridge gone'))
      .mockResolvedValueOnce(() => {});

    await expect(subscribeToTailscaleServeProgress()).resolves.toBeUndefined();
    await subscribeToTailscaleServeProgress();

    expect(mockListen).toHaveBeenCalledTimes(2);
  });
});
