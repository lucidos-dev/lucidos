import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { restoreTargetFor, uploadRestore } from '../restoreUpload';
import { restoreButtonLabel, type RestoreUpload } from '../workspaceForms';
import { fakeTusServer, fileOf, runUntilSettled, useUploadFakeTimers } from '../../../api/client/__tests__/fakeTusServer';
import { installXhrOverFetch, uninstallXhrOverFetch } from '../../../api/client/__tests__/xhrOverFetch';
import { UPLOAD_STALLED_MESSAGE } from '../../../api/client/uploadProgress';

const originalFetch = globalThis.fetch;

function start(server: ReturnType<typeof fakeTusServer>, bytes = 1000) {
  globalThis.fetch = server.fetch;
  const controller = new AbortController();
  const shown: RestoreUpload[] = [];
  const shownAt: number[] = [];
  const outcome = uploadRestore(
    { file: fileOf(bytes), key: 'a-key', name: 'myws' },
    controller.signal,
    (u) => { shown.push(u); shownAt.push(Date.now()); },
  );
  outcome.catch(() => {});
  const labels = () => shown.map(restoreButtonLabel);
  const last = () => shown[shown.length - 1];
  return { shown, shownAt, labels, last, outcome, controller };
}

beforeEach(() => {
  localStorage.clear();
  installXhrOverFetch();
});
afterEach(() => {
  uninstallXhrOverFetch();
  globalThis.fetch = originalFetch;
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('the restore always ends in progress, success or a named error', () => {
  it('shows upload progress, then Starting, then returns the started restore', async () => {
    const server = fakeTusServer();
    const { labels, last, outcome } = start(server);
    expect(await outcome).toEqual({ id: 'myws', name: 'myws' });
    expect(labels()).toContain('Uploading 100%');
    expect(labels()).toContain('Starting…');
    expect(last()).toEqual({ kind: 'idle' });
    const commit = server.requests.find((r) => r.url.endsWith('/workspaces/restore'));
    expect(commit?.method).toBe('POST');
  });

  it('names the proxy cap when a proxy answers 413 to every size, never Starting', async () => {
    const { labels, last, outcome } = start(fakeTusServer({ capBytes: 0 }));
    await expect(outcome).rejects.toThrow(/proxy .* refuses even 512 KiB requests \(HTTP 413\)/);
    expect(labels()).not.toContain('Starting…');
    expect(last()).toEqual({ kind: 'idle' });
  });

  it('shows each retry by name after a dropped connection, then finishes', async () => {
    useUploadFakeTimers();
    const { shown, outcome } = start(fakeTusServer({ dropPatches: 1 }));
    await runUntilSettled(outcome);
    expect(shown.some((u) => u.kind === 'retrying' && u.reason === 'Lucidos could not be reached')).toBe(true);
    expect(await outcome).toEqual({ id: 'myws', name: 'myws' });
  });

  it('a stall is named within 30 s, and is the error once the retries run out', async () => {
    useUploadFakeTimers();
    const began = Date.now();
    const { shown, shownAt, outcome } = start(fakeTusServer({ stallPatches: true }));
    await runUntilSettled(outcome);
    const firstRetry = shown.findIndex((u) => u.kind === 'retrying' && u.reason === UPLOAD_STALLED_MESSAGE);
    expect(firstRetry).toBeGreaterThan(-1);
    expect(shownAt[firstRetry] - began).toBeLessThanOrEqual(30_000);
    await expect(outcome).rejects.toThrow(UPLOAD_STALLED_MESSAGE);
  });

  it('a cancel ends quietly and discards the staged upload', async () => {
    useUploadFakeTimers();
    const server = fakeTusServer({ stallPatches: true });
    const { last, outcome, controller } = start(server);
    await vi.advanceTimersByTimeAsync(1_000);
    controller.abort();
    expect(await outcome).toBeNull();
    expect(last()).toEqual({ kind: 'idle' });
    expect(server.requests[server.requests.length - 1]?.method).toBe('DELETE');
    expect(server.sessions.size).toBe(0);
  });
});

describe('a restore onto a remote gateway goes through the control hop', () => {
  it('uploads and commits under the remote gateway\'s hop base', async () => {
    const server = fakeTusServer();
    globalThis.fetch = server.fetch;
    const started = await uploadRestore(
      { file: fileOf(1000), key: 'a-key', name: 'myws' },
      new AbortController().signal,
      () => {},
      restoreTargetFor('lab'),
    );
    expect(started).toEqual({ id: 'myws', name: 'myws' });
    const hop = '/~/api/v1/remote-gateways/lab/control/';
    expect(server.requests.length).toBeGreaterThan(1);
    expect(server.requests.every((r) => r.url.startsWith(hop))).toBe(true);
    expect(server.requests.some((r) => r.url === `${hop}workspaces/restore`)).toBe(true);
  });

  it('names the missing control grant when the remote gateway refuses', async () => {
    globalThis.fetch = (async () => new Response(
      JSON.stringify({ error: 'control not granted', reason_kind: 'control-not-granted' }),
      { status: 403, headers: { 'content-type': 'application/json' } },
    )) as typeof globalThis.fetch;
    await expect(uploadRestore(
      { file: fileOf(1000), key: 'a-key', name: 'myws' },
      new AbortController().signal,
      () => {},
      restoreTargetFor('lab'),
    )).rejects.toThrow(/lab has not allowed this gateway/);
  });
});
