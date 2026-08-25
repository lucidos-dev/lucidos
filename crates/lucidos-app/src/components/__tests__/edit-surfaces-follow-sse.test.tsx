// @vitest-environment jsdom
/**
 * The trigger detail page was the reported surface. These are the other two
 * that held the same shape: a draft copied out of an entity at mount, keyed on
 * that entity's id, so a later frame for the SAME entity reached nothing.
 *
 * Each surface gets the pair that matters. An untouched field follows the
 * frame. A touched one keeps the user's work. See ADR 0118.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

vi.mock('../../api/client', async () => {
  const actual = await vi.importActual<Record<string, unknown>>('../../api/client');
  return { ...actual, readAppSourceApi: vi.fn(async () => ({ files: [] })) };
});

import { AppUiEditModal } from '../apps/AppUiEditModal';
import { TriggerGroupHeader } from '../triggers/TriggerGroupHeader';
import { appsList, panelOverlay, collapsedTriggerGroupIds } from '../../store/store';
import type { App, TriggerGroup } from '../../store/types';

const APP_ID = 'habit-tracker';
const GROUP_ID = 'group-nightly';

function app(over: Partial<App> = {}): App {
  return { id: APP_ID, name: 'Habit Tracker', description: 'Tracks habits', ...over };
}

function group(over: Partial<TriggerGroup> = {}): TriggerGroup {
  return {
    id: GROUP_ID,
    name: 'Nightly CI & Learning',
    order: 0,
    created: '2026-01-01T00:00:00Z',
    member_count: 2,
    ...over,
  };
}

async function waitFor(done: () => boolean, budgetMs = 1000): Promise<void> {
  for (let waited = 0; waited < budgetMs; waited += 10) {
    if (done()) return;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

function typeInto(el: HTMLInputElement | HTMLTextAreaElement, value: string): void {
  el.value = value;
  el.dispatchEvent(new Event('input', { bubbles: true }));
}

describe('the app edit form follows the AppUpdated frame', () => {
  let host: HTMLElement;

  function nameInput(): HTMLInputElement {
    const el = host.querySelector<HTMLInputElement>('input[type="text"]');
    if (!el) throw new Error('the app name field is not rendered');
    return el;
  }

  beforeEach(() => {
    document.body.innerHTML = '';
    host = document.createElement('div');
    document.body.appendChild(host);
    appsList.value = { status: 'loaded', data: [app()] };
    panelOverlay.value = { type: 'form', form: { type: 'app-edit', appId: APP_ID } };
  });

  afterEach(() => {
    render(null, host);
    document.body.innerHTML = '';
    panelOverlay.value = null;
    appsList.value = { status: 'not-loaded' };
    vi.restoreAllMocks();
  });

  it('repaints the name an untouched form is showing', async () => {
    render(<AppUiEditModal />, host);
    expect(nameInput().value).toBe('Habit Tracker');

    appsList.value = { status: 'loaded', data: [app({ name: 'Habit Coach' })] };
    await waitFor(() => nameInput().value === 'Habit Coach');

    expect(nameInput().value).toBe('Habit Coach');
  });

  it('keeps a name the user typed', async () => {
    render(<AppUiEditModal />, host);
    typeInto(nameInput(), 'My own name');
    await waitFor(() => nameInput().value === 'My own name');

    appsList.value = { status: 'loaded', data: [app({ name: 'Renamed elsewhere' })] };
    await new Promise((resolve) => setTimeout(resolve, 60));

    expect(nameInput().value).toBe('My own name');
  });
});

describe('the trigger group header follows the TriggerGroupRenamed frame', () => {
  let host: HTMLElement;

  function renameField(): HTMLInputElement {
    const el = host.querySelector<HTMLInputElement>('.trigger-group-name-input');
    if (!el) throw new Error('the rename field is not rendered');
    return el;
  }

  beforeEach(() => {
    document.body.innerHTML = '';
    host = document.createElement('div');
    document.body.appendChild(host);
    collapsedTriggerGroupIds.value = new Set();
  });

  afterEach(() => {
    render(null, host);
    document.body.innerHTML = '';
  });

  it('re-seeds the idle rename field, so the next tap offers the served name', async () => {
    render(<TriggerGroupHeader group={group()} />, host);
    expect(renameField().value).toBe('Nightly CI & Learning');

    // The field stays mounted while the user is not renaming. A frame landing
    // in that window used to leave the old name loaded in it.
    render(<TriggerGroupHeader group={group({ name: 'Nightly CI' })} />, host);
    await waitFor(() => renameField().value === 'Nightly CI');

    expect(host.querySelector('.trigger-group-name')?.textContent).toBe('Nightly CI');
    expect(renameField().value).toBe('Nightly CI');
  });
});
