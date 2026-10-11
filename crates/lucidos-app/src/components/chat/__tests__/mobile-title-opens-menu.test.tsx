// @vitest-environment jsdom
/**
 * On a phone the thread title is its own menu button. The row draws no pin and
 * no ⋯: a tap on the title toggles the thread menu, and a hold opens it.
 *
 * Rendered rather than poked through props, because the composition is the
 * thing under test. The hold has to swallow its own lift. The title has to be
 * the overlay's anchor, so a second tap closes. And Pin has to reach the menu,
 * now that no button carries it.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { MobileThreadTitleBar } from '../../layout/MobileAppHeader';
import { ThreadTitlePin } from '../ThreadTitle';
import { threadMap, focusedThreadId } from '../../../store/store';
import { viewportIsMobile } from '../../../utils/viewport';
import type { ThreadState, ThreadMeta } from '../../../store/thread-events';

vi.mock('../../../store/actions/threads', () => ({
  focusThread: vi.fn(),
  handleSaveThread: vi.fn(),
  handleUnsaveThread: vi.fn(),
}));
vi.mock('../../../store/actions/thread-loading', () => ({
  loadThreadEvents: vi.fn(),
  loadOlderThreads: vi.fn(),
  reloadAfterFilterChange: vi.fn(),
  filterChangedSinceLoad: () => false,
  ensureThreadInMap: vi.fn(),
}));

import { handleSaveThread } from '../../../store/actions/threads';

const THREAD_ID = 'title-1';
const TITLE = 'Run the nightly build';

function makeThread(state: ThreadMeta['state'] = 'active'): ThreadState {
  const meta: ThreadMeta = {
    id: THREAD_ID,
    title: TITLE,
    channel: 'chat',
    initiator: 'user',
    saved: false,
    createdAt: '2026-05-01T00:00:00Z',
    updatedAt: '2026-05-01T00:00:00Z',
    status: 'idle',
    summaryVersion: 0,
    messageCount: 1,
    section: 'inbox',
    activeChildrenCount: 0,
    totalChildrenCount: 0,
    blockingDescendantCount: 0,
    attentionDescendantCount: 0,
    codingAgentChangeState: { kind: 'none' },
    codingAgentIsExternalRepo: false,
    lastRevivedAt: '',
    state,
    latestTodoList: null,
    liveEventWaitCount: 0,
    liveEventWaits: [],
  };
  return {
    meta,
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: false,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

let host: HTMLDivElement;

function mount(state: ThreadMeta['state'] = 'active'): void {
  viewportIsMobile.value = true;
  threadMap.value = new Map([[THREAD_ID, makeThread(state)]]);
  focusedThreadId.value = THREAD_ID;
  render(<MobileThreadTitleBar />, host);
}

function titleButton(): HTMLButtonElement {
  const el = host.querySelector<HTMLButtonElement>('.mobile-thread-title-row .thread-title-menu');
  expect(el, 'no title menu button rendered').not.toBeNull();
  return el as HTMLButtonElement;
}

/** The open menu lives in a portal, so it is looked up on the document. */
function openMenu(): HTMLElement | null {
  return document.querySelector<HTMLElement>('.thread-overflow-menu');
}

function menuItems(): string[] {
  return Array.from(openMenu()?.querySelectorAll('[role="menuitem"]') ?? [])
    .map((el) => el.textContent?.trim() ?? '');
}

function pointer(type: string, init: { clientX?: number; clientY?: number } = {}): PointerEvent {
  // jsdom has no PointerEvent constructor, so a MouseEvent carrying the same
  // fields stands in. `useLongPress` reads only `button` and `clientX/Y`.
  return new MouseEvent(type, {
    bubbles: true,
    cancelable: true,
    button: 0,
    clientX: init.clientX ?? 0,
    clientY: init.clientY ?? 0,
  }) as unknown as PointerEvent;
}

/** Let Preact commit its microtask rerender, which fake timers do not drive. */
async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

function click(target: HTMLElement): void {
  target.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, detail: 1 }));
}

async function tap(target: HTMLElement): Promise<void> {
  target.dispatchEvent(pointer('pointerdown'));
  target.dispatchEvent(pointer('pointerup'));
  click(target);
  await flush();
}

/** Press, hold past the 450ms threshold, lift, and the lift's paired click. */
async function hold(target: HTMLElement): Promise<void> {
  target.dispatchEvent(pointer('pointerdown'));
  vi.advanceTimersByTime(500);
  target.dispatchEvent(pointer('pointerup'));
  click(target);
  await flush();
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.mocked(handleSaveThread).mockReset();
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  document.querySelectorAll('.thread-overflow-menu').forEach((el) => el.remove());
  viewportIsMobile.value = false;
  focusedThreadId.value = null;
  threadMap.value = new Map();
  vi.useRealTimers();
});

describe('the mobile title row', () => {
  it('draws no pin and no ⋯, only the title as a menu button', () => {
    mount();
    expect(host.querySelector('.pin-thread-btn')).toBeNull();
    const menuButtons = host.querySelectorAll('button[aria-haspopup="menu"]');
    expect(menuButtons).toHaveLength(1);
    expect(menuButtons[0]).toBe(titleButton());
  });

  it('is named by the thread title, with no label overriding it', () => {
    mount();
    const button = titleButton();
    expect(button.textContent).toContain(TITLE);
    expect(button.hasAttribute('aria-label')).toBe(false);
    expect(button.getAttribute('aria-expanded')).toBe('false');
  });
});

describe('a tap', () => {
  it('opens the thread menu and reports it expanded', async () => {
    mount();
    await tap(titleButton());
    expect(openMenu()).not.toBeNull();
    expect(titleButton().getAttribute('aria-expanded')).toBe('true');
  });

  it('makes the title the overlay anchor, since it is a toggle', async () => {
    mount();
    await tap(titleButton());
    expect(titleButton().hasAttribute('data-overlay-anchor')).toBe(true);
  });

  it('closes the menu on a second tap', async () => {
    mount();
    await tap(titleButton());
    await tap(titleButton());
    expect(openMenu()).toBeNull();
  });
});

describe('a hold', () => {
  it('opens the menu, and its lift does not close it again', async () => {
    mount();
    await hold(titleButton());
    expect(openMenu()).not.toBeNull();
  });

  it('opens nothing when the pointer travels: that is a scroll', async () => {
    mount();
    // A pan ends in `pointercancel`, and the browser pairs no click with it.
    const button = titleButton();
    button.dispatchEvent(pointer('pointerdown'));
    button.dispatchEvent(pointer('pointermove', { clientY: 40 }));
    vi.advanceTimersByTime(500);
    button.dispatchEvent(pointer('pointercancel'));
    await flush();
    expect(openMenu()).toBeNull();
  });
});

describe('a right-click', () => {
  it('opens the same menu (ADR 0285)', async () => {
    mount();
    titleButton().dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
    await flush();
    expect(openMenu()).not.toBeNull();
  });

  it('opens one menu when a touch hold also fires contextmenu, as Android does', async () => {
    mount();
    const button = titleButton();
    button.dispatchEvent(pointer('pointerdown'));
    vi.advanceTimersByTime(500);
    button.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
    button.dispatchEvent(pointer('pointerup'));
    click(button);
    await flush();
    expect(document.querySelectorAll('.thread-overflow-menu')).toHaveLength(1);
  });

  it('leaves Option+right-click to the native menu', async () => {
    mount();
    const e = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, altKey: true });
    titleButton().dispatchEvent(e);
    await flush();
    expect(openMenu()).toBeNull();
    expect(e.defaultPrevented).toBe(false);
  });
});

describe('the menu it opens', () => {
  it('offers Pin thread on a pointer open, now that no button carries it', async () => {
    mount();
    await tap(titleButton());
    expect(menuItems()).toContain('Pin thread');
  });

  it('runs the pin from the menu', async () => {
    mount();
    await tap(titleButton());
    const pin = Array.from(openMenu()?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])
      .find((el) => el.textContent?.trim() === 'Pin thread');
    pin?.click();
    expect(handleSaveThread).toHaveBeenCalledWith(THREAD_ID);
  });

  it('leads with Show in Folders', async () => {
    mount();
    await tap(titleButton());
    expect(menuItems()[0]).toBe('Show in Folders');
  });

  it('offers a draft neither Pin nor Show in Folders', async () => {
    mount('composing');
    await tap(titleButton());
    expect(menuItems()).not.toContain('Pin thread');
    expect(menuItems()).not.toContain('Show in Folders');
  });
});

describe('the desktop title row', () => {
  it('keeps its pin', () => {
    viewportIsMobile.value = false;
    render(<ThreadTitlePin thread={makeThread()} />, host);
    expect(host.querySelector('.pin-thread-btn')).not.toBeNull();
  });
});
