// @vitest-environment jsdom
/**
 * The thread ⋯ menu's Add widget (ADR 0466): a step in the same menu that
 * lists the reusable widgets, the picker the Home board's Add shows. A pick
 * pins the widget to that thread's shelf. Home and a draft offer none.
 */
import { beforeAll, describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../store/actions/widgets', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/widgets')>()),
  loadReusableWidgets: vi.fn(async () => {}),
}));
vi.mock('../../../store/actions/widget-actions', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/widget-actions')>()),
  pinWidgetToShelf: vi.fn(async () => {}),
}));

import { ThreadOverflowItemsChunk, ThreadOverflowMenu } from '../ThreadOverflowMenu';
import { pinWidgetToShelf } from '../../../store/actions/widget-actions';
import { reusableWidgets } from '../../../store/widgets';
import { dismissTopOverlay } from '../../../store/overlayStack';
import { threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import type { App } from '../../../store/types';

beforeAll(() => ThreadOverflowItemsChunk.preload());

let host: HTMLElement | null = null;

function openMenu(): void {
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => { render(<ThreadOverflowMenu threadId="t" title="T" />, host!); });
  act(() => { host!.querySelector<HTMLElement>('button[aria-haspopup="menu"]')?.click(); });
}

const rows = () => [...document.querySelectorAll<HTMLElement>('.thread-overflow-menu [role="menuitem"]')];
const row = (label: string) => rows().find((el) => el.textContent?.trim() === label);
/** A picker row also holds its icon's monogram and its description. */
const widgetRow = (id: string) => document.querySelector<HTMLElement>(`.thread-overflow-menu [data-add-widget="${id}"]`);

const clock = { id: 'clock', name: 'Clock', description: 'The time', kind: 'widget', reusable: true, reveal: 'on-load' } as App;

beforeEach(() => {
  vi.clearAllMocks();
  reusableWidgets.value = { status: 'loaded', data: [clock] };
});

afterEach(() => {
  if (host) { render(null, host); host.remove(); host = null; }
  document.querySelectorAll('.thread-overflow-menu').forEach((el) => el.remove());
  threadMap.value = new Map();
});

describe('Add widget in the thread overflow menu', () => {
  it('steps into the picker and pins the pick to this thread', () => {
    threadMap.value = new Map([['t', makeThreadState('t')]]);
    openMenu();
    act(() => { row('Add widget…')!.click(); });
    expect(row('Add widget…')).toBeUndefined();
    expect(document.activeElement?.textContent).toBe('Add widget');
    act(() => { widgetRow('clock')!.click(); });
    expect(pinWidgetToShelf).toHaveBeenCalledWith({ appId: 'clock', threadId: 't' }, 'Clock');
  });

  it('steps back on Escape, and on its Back row', () => {
    threadMap.value = new Map([['t', makeThreadState('t')]]);
    openMenu();
    act(() => { row('Add widget…')!.click(); });
    act(() => { dismissTopOverlay(); });
    expect(row('Add widget…')).toBeTruthy();
    act(() => { row('Add widget…')!.click(); });
    act(() => { row('Add widget')!.click(); });
    expect(widgetRow('clock')).toBeNull();
  });

  it('offers none in Home, whose board has its own Add, or on a draft', () => {
    threadMap.value = new Map([['t', makeThreadState('t', { meta: { home: true } })]]);
    openMenu();
    expect(row('Add widget…')).toBeUndefined();
    act(() => { render(null, host!); });
    document.querySelectorAll('.thread-overflow-menu').forEach((el) => el.remove());
    threadMap.value = new Map([['t', makeThreadState('t', { meta: { state: 'composing' } })]]);
    openMenu();
    expect(row('Add widget…')).toBeUndefined();
  });
});
