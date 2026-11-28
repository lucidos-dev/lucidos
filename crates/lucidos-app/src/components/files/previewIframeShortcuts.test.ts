import { describe, it, expect, beforeEach } from 'vitest';
import { bridgePreviewIframeShortcuts } from './previewIframeShortcuts';
import { focusedPane, splitRatio } from '../../store/store';

function fakeContentDoc() {
  const handlers: Record<string, { fn: (e: unknown) => void; capture: boolean }[]> = {};
  return {
    addEventListener: (type: string, fn: (e: unknown) => void, capture?: boolean) => {
      (handlers[type] ??= []).push({ fn, capture: capture === true });
    },
    removeEventListener: () => {},
    listenerCount: (type: string) => (handlers[type] ?? []).length,
    allCapture: (type: string) => (handlers[type] ?? []).every((h) => h.capture),
    fire: (type: string, e: unknown) => { for (const h of handlers[type] ?? []) h.fn(e); },
  };
}
type FakeDoc = ReturnType<typeof fakeContentDoc>;
const iframeWith = (doc: FakeDoc | null) => ({ contentDocument: doc } as unknown as HTMLIFrameElement);

function chord(over: Partial<{ metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean; key: string }>) {
  const e = { metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, key: '', defaultPrevented: false, ...over };
  (e as unknown as { preventDefault: () => void }).preventDefault = () => { e.defaultPrevented = true; };
  return e;
}

beforeEach(() => {
  (globalThis as { innerWidth: number }).innerWidth = 1024;
  focusedPane.value = 'thread';
  splitRatio.value = 0.5;
});

describe('bridgePreviewIframeShortcuts', () => {
  it('registers a single capture-phase keydown listener on the preview document', () => {
    const doc = fakeContentDoc();
    bridgePreviewIframeShortcuts(iframeWith(doc));
    expect(doc.listenerCount('keydown')).toBe(1);
    expect(doc.allCapture('keydown')).toBe(true);
  });

  it('routes a ⌘⇧↵ keydown in the preview document to the host maximize action', () => {
    const doc = fakeContentDoc();
    bridgePreviewIframeShortcuts(iframeWith(doc));

    const e = chord({ key: 'Enter', metaKey: true, shiftKey: true });
    doc.fire('keydown', e);

    expect(e.defaultPrevented).toBe(true);
    expect(focusedPane.value).toBe('content');
    expect(splitRatio.value).toBe(0);
  });

  it('attaches once per document — a second bridge call does not stack listeners', () => {
    const doc = fakeContentDoc();
    bridgePreviewIframeShortcuts(iframeWith(doc));
    bridgePreviewIframeShortcuts(iframeWith(doc));
    expect(doc.listenerCount('keydown')).toBe(1);
  });

  it('leaves non-shortcut keys alone so the preview keeps its own behavior', () => {
    const doc = fakeContentDoc();
    bridgePreviewIframeShortcuts(iframeWith(doc));

    const e = chord({ key: 'a' });
    doc.fire('keydown', e);

    expect(e.defaultPrevented).toBe(false);
    expect(splitRatio.value).toBe(0.5);
  });

  it('no-ops on a cross-origin preview (contentDocument access throws)', () => {
    const crossOrigin = { get contentDocument(): Document { throw new Error('cross-origin'); } } as unknown as HTMLIFrameElement;
    expect(() => bridgePreviewIframeShortcuts(crossOrigin)).not.toThrow();
  });

  it('no-ops on a null iframe', () => {
    expect(() => bridgePreviewIframeShortcuts(null)).not.toThrow();
  });
});
