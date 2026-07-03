import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { json, API } from './_core';
import { engineRestarting } from '../../store/store';

// While the engine restarts (Apply & Restart) every connection is dropped, so a
// GET fired in that window hits a dead socket and surfaces as
// `TypeError: Load failed` — which the page behind the "Restarting engine…"
// overlay paints as a spurious "Failed to load…" error. `_core` holds GET reads
// until the restart completes; these tests pin that gate, plus its two
// exemptions (the health probe and mutations).

const originalFetch = globalThis.fetch;

function okJson(): Response {
  return new Response(JSON.stringify({ ok: true }), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
}

describe('read gate during engine restart', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn().mockImplementation(() => Promise.resolve(okJson()));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    engineRestarting.value = false;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    engineRestarting.value = false;
    vi.restoreAllMocks();
  });

  it('holds a GET while restarting, then runs it once the restart completes', async () => {
    engineRestarting.value = true;
    let settled = false;
    const p = json(`${API}/changes`).then(() => { settled = true; });

    // Let microtasks drain — the read must NOT touch the network mid-restart.
    await Promise.resolve();
    await Promise.resolve();
    expect(mockFetch).not.toHaveBeenCalled();
    expect(settled).toBe(false);

    // Watchdog flips the flag on reconnect; the queued read now runs.
    engineRestarting.value = false;
    await p;
    expect(mockFetch).toHaveBeenCalledTimes(1);
    expect(settled).toBe(true);
  });

  it('does NOT gate the health probe — it must run so the watchdog can detect the engine returned', async () => {
    engineRestarting.value = true;
    await json(`${API}/health`);
    expect(mockFetch).toHaveBeenCalledTimes(1);
    expect(mockFetch.mock.calls[0][0]).toContain('/health');
  });

  it('does NOT gate mutations (a non-GET through json) — only reads queue', async () => {
    engineRestarting.value = true;
    await json(`${API}/some/mutation`, { method: 'POST' });
    expect(mockFetch).toHaveBeenCalledTimes(1);
  });

  it('runs GET reads immediately when not restarting', async () => {
    await json(`${API}/changes`);
    expect(mockFetch).toHaveBeenCalledTimes(1);
  });
});
