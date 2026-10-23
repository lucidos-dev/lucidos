// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

const showToast = vi.hoisted(() => vi.fn());
vi.mock('../store/store', () => ({ showToast }));

const { copyImageToClipboard, fetchImageAsPng } = await import('./clipboard');
const { clipboardAbilitiesOf } = await import('./platform');

/** Records what it was built with, as the real `ClipboardItem` holds it. */
class FakeClipboardItem {
  constructor(readonly items: Record<string, Promise<Blob>>) {}
}

const png = new Blob(['png-bytes'], { type: 'image/png' });

function stubFetch(body: Blob, ok = true) {
  vi.stubGlobal('fetch', vi.fn(async () => ({ ok, status: ok ? 200 : 404, blob: async () => body })));
}

function stubClipboard(write: (items: FakeClipboardItem[]) => Promise<void>) {
  vi.stubGlobal('ClipboardItem', FakeClipboardItem);
  Object.defineProperty(navigator, 'clipboard', { value: { write }, configurable: true });
}

/** Lets the write's `then` and the fetch it waits on run. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => showToast.mockReset());
afterEach(() => {
  vi.unstubAllGlobals();
  Object.defineProperty(navigator, 'clipboard', { value: undefined, configurable: true });
});

describe('the image clipboard ability', () => {
  it('needs both clipboard.write and ClipboardItem', () => {
    expect(clipboardAbilitiesOf({ write: async () => {} }, FakeClipboardItem).copyImage).toBe(true);
    expect(clipboardAbilitiesOf({ write: async () => {} }, undefined).copyImage).toBe(false);
    expect(clipboardAbilitiesOf({ writeText: async () => {} }, FakeClipboardItem).copyImage).toBe(false);
    expect(clipboardAbilitiesOf(undefined, FakeClipboardItem).copyImage).toBe(false);
  });
});

describe('fetchImageAsPng', () => {
  it('passes a PNG through byte for byte', async () => {
    stubFetch(png);
    expect(await fetchImageAsPng('/data/artifacts/shot.png')).toBe(png);
  });

  it('fails on an HTTP error rather than copying the error page', async () => {
    stubFetch(new Blob(['nope'], { type: 'text/html' }), false);
    await expect(fetchImageAsPng('/data/artifacts/gone.png')).rejects.toThrow('HTTP 404');
  });
});

describe('copyImageToClipboard', () => {
  it('writes the image as a PNG item and says so', async () => {
    stubFetch(png);
    const write = vi.fn(async () => {});
    stubClipboard(write);

    copyImageToClipboard('/data/artifacts/shot.png');
    // The item is built in the tap's own task, before the fetch has finished.
    expect(write).toHaveBeenCalledTimes(1);
    const [[[item]]] = write.mock.calls as unknown as [[FakeClipboardItem[]]];
    expect(Object.keys(item.items)).toEqual(['image/png']);
    expect(await item.items['image/png']).toBe(png);

    await settle();
    expect(showToast).toHaveBeenCalledWith('Image copied', 'success');
  });

  it('names a refused write', async () => {
    stubFetch(png);
    stubClipboard(async () => { throw new Error('denied'); });

    copyImageToClipboard('/data/artifacts/shot.png');
    await settle();
    expect(showToast).toHaveBeenCalledWith('Failed to copy image: denied', 'error');
  });

  it('names the fetch failure, not the generic write error it caused', async () => {
    stubFetch(new Blob(['nope'], { type: 'text/html' }), false);
    stubClipboard(async (items) => {
      await items[0].items['image/png'].catch(() => {});
      throw new Error('Document is not focused');
    });

    copyImageToClipboard('/data/artifacts/gone.png');
    await settle();
    expect(showToast).toHaveBeenCalledWith('Failed to copy image: HTTP 404', 'error');
  });

  it('reports a client with no image clipboard instead of failing silently', () => {
    copyImageToClipboard('/data/artifacts/shot.png');
    expect(showToast).toHaveBeenCalledWith('No clipboard access on this address.', 'error');
  });
});
