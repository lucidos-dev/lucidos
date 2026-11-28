import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { landingLaneCanceling, mainCheckClosing, toasts } from '../store';
import { cancelLaneBatch, closeMainCheck, removeFromLandingLane } from './laneControls';

describe('Remove and Cancel', () => {
  const originalFetch = globalThis.fetch;
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn().mockResolvedValue(new Response(JSON.stringify({ message: 'ok' }), { status: 200 }));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    landingLaneCanceling.value = null;
    toasts.value = [];
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it('Remove deletes the change from the lane queue', async () => {
    await removeFromLandingLane('c-2');
    const [url, init] = mockFetch.mock.calls[0];
    expect(url).toBe('/api/v1/changes/c-2/queue');
    expect(init.method).toBe('DELETE');
  });

  it('a refused Remove says why', async () => {
    mockFetch.mockResolvedValue(new Response(
      JSON.stringify({ error: 'This change is landing in a batch. Cancel the batch to take it out.' }),
      { status: 409 },
    ));
    await removeFromLandingLane('c-1');
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain('Cancel the batch to take it out.');
  });

  it('Cancel posts the batch id, and reads as canceling until the batch closes', async () => {
    await cancelLaneBatch('b-1');
    const [url, init] = mockFetch.mock.calls[0];
    expect(url).toBe('/api/v1/changes/lane/cancel');
    expect(init.method).toBe('POST');
    expect(JSON.parse(init.body as string)).toEqual({ batch_id: 'b-1' });
    expect(landingLaneCanceling.value).toBe('b-1');
  });

  it('a failed Cancel gives the button back', async () => {
    mockFetch.mockResolvedValue(new Response(JSON.stringify({ error: 'No lane batch with that id is resolving.' }), { status: 400 }));
    await cancelLaneBatch('b-1');
    expect(landingLaneCanceling.value).toBeNull();
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain('No lane batch with that id is resolving.');
  });
});

describe('Close', () => {
  const originalFetch = globalThis.fetch;
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn().mockResolvedValue(new Response(JSON.stringify({ message: 'ok' }), { status: 200 }));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    mainCheckClosing.value = null;
    toasts.value = [];
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it('posts the close, and reads as closing until the red closes', async () => {
    await closeMainCheck('k-1');
    const [url, init] = mockFetch.mock.calls[0];
    expect(url).toBe('/api/v1/main-check/close');
    expect(init.method).toBe('POST');
    expect(mainCheckClosing.value).toBe('k-1');
  });

  it('a refused Close gives the button back and says why', async () => {
    mockFetch.mockResolvedValue(new Response(
      JSON.stringify({ error: 'Main is not red, so there is nothing to close.' }),
      { status: 409 },
    ));
    await closeMainCheck('k-1');
    expect(mainCheckClosing.value).toBeNull();
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain('nothing to close');
  });
});
