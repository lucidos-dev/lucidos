// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

import { ArrivalList, ARRIVAL_CHECK_SLACK_MS } from '../ArrivalList';
import { mergeOrder, useArrivals } from '../arrivals';
import { applyNavFocus, clearNavFocus, hasNavFocus, NAV_FOCUS_FADE_MS, NAV_FOCUS_HOLD_MS, NAV_FOCUS_RAMP_MS } from '../focusMarker';
import { DISCLOSURE_MAX_MS } from '../../../utils/disclosureMotion';
import { motionPreference } from '../../../utils/motion';

interface FakeAnimation { el: HTMLElement; finish: () => void; cancel: () => void; finished: Promise<void> }
let anims: FakeAnimation[] = [];
let onScreen = true;
let tabHidden = false;
const originalRect = HTMLElement.prototype.getBoundingClientRect;
let host: HTMLElement;

function stubBrowser() {
  anims = [];
  onScreen = true;
  tabHidden = false;
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => (tabHidden ? 'hidden' : 'visible') });
  (HTMLElement.prototype as unknown as { animate: unknown }).animate = function (this: HTMLElement) {
    let resolve!: () => void;
    let reject!: (e: unknown) => void;
    const finished = new Promise<void>((res, rej) => { resolve = res; reject = rej; });
    finished.catch(() => {});
    const anim = { el: this, finished, finish: () => resolve(), cancel: () => reject(new DOMException('', 'AbortError')) };
    anims.push(anim);
    return anim;
  };
  HTMLElement.prototype.getBoundingClientRect = function () {
    return { top: 0, bottom: 40, height: 40 } as DOMRect;
  };
  (globalThis as unknown as { IntersectionObserver: unknown }).IntersectionObserver = class {
    constructor(private callback: IntersectionObserverCallback) {}
    observe(el: Element) {
      if (tabHidden) return;
      this.callback([{ target: el, isIntersecting: onScreen } as IntersectionObserverEntry], this as unknown as IntersectionObserver);
    }
    disconnect() {}
  };
}

function View({ keys, open = true }: { keys: string[] | null; open?: boolean }) {
  const arrived = useArrivals(keys);
  if (!keys || !open) return null;
  return (
    <ArrivalList items={keys} keyOf={(k) => k} arrived={arrived}>
      {(k, marker) => <div class={marker ? `row ${marker}` : 'row'} data-key={k}>{k}</div>}
    </ArrivalList>
  );
}

async function show(keys: string[] | null, open = true) {
  await act(() => { render(<View keys={keys} open={open} />, host); });
}

async function unmount() {
  await act(() => { render(null, host); });
}

const row = (key: string) => host.querySelector<HTMLElement>(`.row[data-key="${key}"]`);
const marked = (key: string) => !!row(key)?.classList.contains('arrival-marker');
const fading = (key: string) => !!row(key)?.classList.contains('arrival-marker-fading');
const rolled = (key: string) => anims.some(a => a.el.contains(row(key)));

async function finishRolls() {
  await act(async () => {
    for (const a of anims) a.finish();
    await Promise.resolve();
    await Promise.resolve();
  });
}

async function press() {
  await act(() => { document.dispatchEvent(new KeyboardEvent('keydown', { key: 'j' })); });
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  motionPreference.value = 'full';
  stubBrowser();
});

afterEach(() => {
  render(null, host);
  host.remove();
  clearNavFocus();
  vi.useRealTimers();
  delete (HTMLElement.prototype as unknown as { animate?: unknown }).animate;
  delete (globalThis as unknown as { IntersectionObserver?: unknown }).IntersectionObserver;
  delete (document as unknown as { visibilityState?: unknown }).visibilityState;
  HTMLElement.prototype.getBoundingClientRect = originalRect;
});

describe('mergeOrder', () => {
  const e = (key: string) => ({ key, item: key });

  it('keeps a departing row where it was', () => {
    expect(mergeOrder([e('a'), e('b'), e('c')], [e('a'), e('c')]).map(x => x.key)).toEqual(['a', 'b', 'c']);
  });

  it('keeps a departing first row first', () => {
    expect(mergeOrder([e('a'), e('b')], [e('b')]).map(x => x.key)).toEqual(['a', 'b']);
  });

  it('takes the live order and items for rows that stay', () => {
    const live = [{ key: 'b', item: 'B' }, { key: 'a', item: 'A' }];
    expect(mergeOrder([e('a'), e('b')], live)).toEqual(live);
  });
});

describe('ArrivalList', () => {
  it('shows the current state as it loads, with no roll and no highlight', async () => {
    await show(null);
    await show(['a', 'b']);
    expect(row('a')).not.toBeNull();
    expect(marked('a') || marked('b')).toBe(false);
    expect(anims).toHaveLength(0);
  });

  it('rolls in and highlights a row that arrives after that', async () => {
    await show(['a']);
    await show(['b', 'a']);
    expect(rolled('b')).toBe(true);
    expect(marked('b')).toBe(true);
    expect(rolled('a') || marked('a')).toBe(false);
  });

  it('highlights nothing that arrived while the view was closed', async () => {
    await show(['a']);
    await unmount();
    await show(['a', 'b']);
    expect(marked('a') || marked('b')).toBe(false);
  });

  it('highlights nothing when a folded list opens', async () => {
    await show(['a']);
    await show(['a', 'b'], false);
    await show(['a', 'b']);
    expect(marked('a') || marked('b')).toBe(false);
    expect(anims).toHaveLength(0);
  });

  it('highlights a row that arrives as its list appears', async () => {
    await show([], false);
    await show(['a']);
    expect(marked('a')).toBe(true);
  });

  it('takes no arrival from a reload', async () => {
    await show(['a']);
    await show(null);
    await show(['a']);
    expect(marked('a')).toBe(false);
  });

  it('rolls a leaving row out, inert, and then drops it', async () => {
    await show(['a', 'b']);
    await show(['a']);
    expect(row('b')).not.toBeNull();
    expect(row('b')!.closest('[inert]')).not.toBeNull();
    await finishRolls();
    expect(row('b')).toBeNull();
    expect(row('a')).not.toBeNull();
  });

  it('highlights a row that comes back before it has rolled out', async () => {
    await show(['a', 'b']);
    await show(['a']);
    await show(['a', 'b']);
    expect(marked('b')).toBe(true);
  });

  it('never takes the navigation landing', async () => {
    const landing = document.createElement('div');
    document.body.appendChild(landing);
    applyNavFocus(landing);
    await show(['a']);
    await show(['a', 'b']);
    expect(marked('b')).toBe(true);
    expect(hasNavFocus()).toBe(true);
    landing.remove();
  });

  describe('the highlight', () => {
    beforeEach(() => {
      vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    });

    const landed = () => DISCLOSURE_MAX_MS + ARRIVAL_CHECK_SLACK_MS;
    const hold = () => NAV_FOCUS_RAMP_MS + NAV_FOCUS_HOLD_MS;

    it('waits past the longest disclosure roll before checking visibility', async () => {
      onScreen = false;
      await show(['a']);
      await show(['a', 'b']);
      await act(() => { vi.advanceTimersByTime(DISCLOSURE_MAX_MS); });
      expect(marked('b')).toBe(true);
      await act(() => { vi.advanceTimersByTime(ARRIVAL_CHECK_SLACK_MS); });
      expect(marked('b')).toBe(false);
    });

    it('goes out unseen when the row lands off screen', async () => {
      onScreen = false;
      await show(['a']);
      await show(['a', 'b']);
      expect(marked('b')).toBe(true);
      await act(() => { vi.advanceTimersByTime(landed()); });
      expect(marked('b')).toBe(false);
      expect(fading('b')).toBe(false);
    });

    it('goes out unseen when the row lands in a hidden tab', async () => {
      await show(['a']);
      tabHidden = true;
      await show(['a', 'b']);
      await act(() => { vi.advanceTimersByTime(landed()); });
      tabHidden = false;
      expect(marked('b')).toBe(false);
    });

    it('holds where it is seen, then dissolves on the next action', async () => {
      await show(['a']);
      await show(['a', 'b']);
      await act(() => { vi.advanceTimersByTime(landed()); });
      await press();
      expect(fading('b')).toBe(false);
      await act(() => { vi.advanceTimersByTime(hold()); });
      expect(fading('b')).toBe(true);
      await act(() => { vi.advanceTimersByTime(NAV_FOCUS_FADE_MS); });
      expect(marked('b')).toBe(false);
    });

    it('still shows under reduced motion, and clears with no dissolve', async () => {
      motionPreference.value = 'reduce';
      await show(['a']);
      await show(['a', 'b']);
      expect(marked('b')).toBe(true);
      expect(anims).toHaveLength(0);
      await act(() => { vi.advanceTimersByTime(hold()); });
      expect(marked('b')).toBe(true);
      await press();
      expect(marked('b')).toBe(false);
    });
  });
});
