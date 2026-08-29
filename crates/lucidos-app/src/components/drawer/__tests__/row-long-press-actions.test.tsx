// @vitest-environment jsdom
/**
 * A mobile drawer row has no ⋯, and a long press opens its actions instead.
 *
 * The trigger is a 31x27px box against the pane's right edge, the hardest place
 * on a phone to reach. Its menu opens in the same corner. So the mobile row
 * drops it and the whole row becomes the target.
 *
 * Rendered rather than poked through props, because the composition IS the
 * thing under test. The hold has to swallow its own paired click. The prefetch
 * has to survive that composition. And the trigger's absence is what publishes
 * the opener the hold calls.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { ThreadRow, ComposingThreadRow } from '../ThreadDrawer';
import { threadMap } from '../../../store/store';
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

import { focusThread } from '../../../store/actions/threads';
import { loadThreadEvents } from '../../../store/actions/thread-loading';

const THREAD_ID = 'row-1';

function makeThread(id: string): ThreadState {
  const meta: ThreadMeta = {
    id,
    title: 'A thread',
    channel: 'chat',
    initiator: 'user',
    saved: false,
    createdAt: '2026-05-01T00:00:00Z',
    updatedAt: '2026-05-01T00:00:00Z',
    status: 'idle',
    messageCount: 1,
    section: 'inbox',
    activeChildrenCount: 0,
    totalChildrenCount: 0,
    blockingDescendantCount: 0,
    attentionDescendantCount: 0,
    codingAgentHasDiff: false,
    codingAgentProposed: false,
    codingAgentRequiresRestart: false,
    codingAgentIsExternalRepo: false,
    codingAgentApplying: false,
    lastRevivedAt: '',
    state: 'active',
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

function mount(mobile: boolean): void {
  viewportIsMobile.value = mobile;
  render(<ThreadRow threadId={THREAD_ID} status="idle" />, host);
}

function row(): HTMLElement {
  const el = host.querySelector<HTMLElement>(`[data-thread-nav="${THREAD_ID}"]`);
  expect(el, 'no thread row rendered').not.toBeNull();
  return el as HTMLElement;
}

function overflowTrigger(): HTMLElement | null {
  return host.querySelector<HTMLElement>('button[aria-haspopup="menu"]');
}

/** The open menu lives in a portal outside `host`, so it is looked up on the
 *  document rather than in the render container. */
function openMenu(): HTMLElement | null {
  return document.querySelector<HTMLElement>('.thread-overflow-menu');
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

/** Let Preact commit. It schedules a rerender on a microtask, and fake timers
 *  do not drive those. Without this an assertion right after a state change
 *  reads the previous frame. */
async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

/** Press, hold past the 450ms threshold, lift. The click the browser pairs
 *  with the lift is dispatched too: swallowing it is the contract. */
async function hold(target: HTMLElement, moveTo?: { x: number; y: number }): Promise<void> {
  target.dispatchEvent(pointer('pointerdown'));
  if (moveTo) target.dispatchEvent(pointer('pointermove', { clientX: moveTo.x, clientY: moveTo.y }));
  vi.advanceTimersByTime(500);
  target.dispatchEvent(pointer('pointerup'));
  target.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
  await flush();
}

async function tap(target: HTMLElement): Promise<void> {
  target.dispatchEvent(pointer('pointerdown'));
  target.dispatchEvent(pointer('pointerup'));
  target.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
  await flush();
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.mocked(focusThread).mockReset();
  vi.mocked(loadThreadEvents).mockReset();
  threadMap.value = new Map([[THREAD_ID, makeThread(THREAD_ID)]]);
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  document.querySelectorAll('.thread-overflow-menu').forEach(el => el.remove());
  viewportIsMobile.value = false;
  vi.useRealTimers();
});

describe('the mobile row drops its ⋯', () => {
  it('renders no overflow trigger', () => {
    mount(true);
    expect(overflowTrigger()).toBeNull();
  });

  it('keeps the pin, the row\'s one remaining control', () => {
    mount(true);
    expect(host.querySelector('.pin-thread-btn')).not.toBeNull();
  });
});

describe('the desktop row keeps its ⋯', () => {
  it('renders exactly one overflow trigger', () => {
    mount(false);
    expect(host.querySelectorAll('button[aria-haspopup="menu"]')).toHaveLength(1);
  });

  it('is what the keyboard shortcut looks up, inside the row', () => {
    mount(false);
    // `openHighlightedThreadActions` finds the trigger by exactly this path.
    const found = host.querySelector(`[data-thread-nav="${THREAD_ID}"] button[aria-haspopup="menu"]`);
    expect(found).not.toBeNull();
  });

  it('does not arm a hold, so holding a row just opens the thread', async () => {
    mount(false);
    await hold(row());
    expect(openMenu()).toBeNull();
    expect(focusThread).toHaveBeenCalledWith(THREAD_ID);
  });
});

describe('the mobile gesture', () => {
  it('opens the actions menu on a hold', async () => {
    mount(true);
    await hold(row());
    expect(openMenu()).not.toBeNull();
  });

  it('does not also focus the thread: the paired click is swallowed', async () => {
    mount(true);
    await hold(row());
    expect(focusThread).not.toHaveBeenCalled();
  });

  it('still focuses the thread on an ordinary tap', async () => {
    mount(true);
    await tap(row());
    expect(openMenu()).toBeNull();
    expect(focusThread).toHaveBeenCalledWith(THREAD_ID);
  });

  it('opens nothing when the pointer travels: that is a scroll', async () => {
    mount(true);
    await hold(row(), { x: 0, y: 40 });
    expect(openMenu()).toBeNull();
  });

  it('keeps the row prefetch on the press', () => {
    mount(true);
    row().dispatchEvent(pointer('pointerdown'));
    expect(loadThreadEvents).toHaveBeenCalledWith(THREAD_ID);
  });

  // A second hold while the menu is up would re-open it the instant the first
  // outside press dismissed it. Nothing in this hook stops that, and nothing
  // needs to: an open overlay inerts the shell behind it, so the row takes no
  // pointer at all. The exemption that would break it is `data-overlay-anchor`,
  // and host-opened mode deliberately marks no anchor. Asserting the mechanism,
  // because jsdom resolves no CSS and cannot show the effect.
  it('exempts nothing from the inert while open, so no second press can arm', async () => {
    mount(true);
    await hold(row());
    expect(document.documentElement.hasAttribute('data-overlay-open')).toBe(true);
    expect(document.querySelector('[data-overlay-anchor]')).toBeNull();
  });

  it('leaves a press that began on the pin to the pin', async () => {
    mount(true);
    const pin = host.querySelector<HTMLElement>('.pin-thread-btn');
    expect(pin, 'no pin button rendered').not.toBeNull();
    await hold(pin as HTMLElement);
    expect(openMenu()).toBeNull();
  });
});

// A compose draft row has no pin, so its ⋯ was the actions box's only child.
// Kept on mobile, that box would be an empty flex item, opening the right
// column's gap under the chips. So the row drops the box with the button.
describe('a compose draft row', () => {
  function mountDraft(mobile: boolean): void {
    viewportIsMobile.value = mobile;
    const draft = makeThread('draft-1');
    draft.meta.state = 'composing';
    render(<ComposingThreadRow thread={draft} />, host);
  }

  it('keeps its ⋯ inside the actions box on desktop', () => {
    mountDraft(false);
    expect(host.querySelectorAll('button[aria-haspopup="menu"]')).toHaveLength(1);
    expect(host.querySelector('.thread-row-actions')).not.toBeNull();
  });

  it('renders neither the ⋯ nor an empty actions box on mobile', () => {
    mountDraft(true);
    expect(host.querySelector('button[aria-haspopup="menu"]')).toBeNull();
    expect(host.querySelector('.thread-row-actions')).toBeNull();
  });

  it('still opens its menu on a hold', async () => {
    mountDraft(true);
    const el = host.querySelector<HTMLElement>('[data-thread-nav="draft-1"]');
    expect(el, 'no draft row rendered').not.toBeNull();
    await hold(el as HTMLElement);
    expect(openMenu()).not.toBeNull();
  });
});
