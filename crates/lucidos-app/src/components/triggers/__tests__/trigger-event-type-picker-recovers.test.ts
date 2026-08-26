// @vitest-environment jsdom
/**
 * The event-type picker on an open trigger detail page must recover from a
 * transport failure.
 *
 * Reported as "Failed to load event types". The engine had answered every
 * request it ever saw, so the failure was transport-side. A reopen already
 * retried. The panel the user was looking at did not, and it offered one
 * empty-valued option that cleared the field.
 *
 * The verdict lives in a module-level cache, so each case starts from a fresh
 * module graph. `vi.resetModules()` plus dynamic imports is what gives one:
 * preact, the store and the component must come from the same graph.
 *
 * See docs/plans/2026-08-24-trigger-event-type-picker-recovers-from-a-transport-failure.md
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { TriggerInfo } from '../../../store/types';

const { fetchEventTypes } = vi.hoisted(() => ({
  fetchEventTypes: vi.fn<() => Promise<string[]>>(),
}));

vi.mock('../../../api/client', async () => {
  const actual = await vi.importActual<Record<string, unknown>>('../../../api/client');
  return { ...actual, fetchEventTypes };
});

import { stubIntentFieldObservers } from './intentFieldStubs';

stubIntentFieldObservers();

const TRIGGER_ID = '220e6c0d-f626-41c3-955c-6b6a66674ce6';

function trigger(): TriggerInfo {
  return {
    id: TRIGGER_ID,
    name: 'Scheduled CI result',
    cron_expressions: [],
    timezone: 'UTC',
    paused: false,
    run: { type: 'intent', intent: 'check CI' },
    on: [{ event_type: 'DeployFinished' }],
  };
}

/** Mount the trigger form on `host`, from one module graph. Returns its
 *  unmount. */
async function mountForm(host: HTMLElement): Promise<() => void> {
  const { h, render } = await import('preact');
  const store = await import('../../../store/store');
  const { TriggerDetails } = await import('../TriggerDetails');

  store.chatModels.value = { status: 'loaded', data: [] };
  store.triggerGroups.value = { status: 'loaded', data: [] };
  store.triggers.value = { status: 'loaded', data: [trigger()] };
  store.panelOverlay.value = { type: 'form', form: { type: 'trigger', triggerId: TRIGGER_ID } };

  render(h(TriggerDetails, {}), host);
  return () => render(null, host);
}

/** Preact runs effects off the render, so poll rather than wait a fixed span:
 *  a loaded parallel run stretches the frames they ride on. */
async function waitFor(done: () => boolean, budgetMs = 1000): Promise<void> {
  for (let waited = 0; waited < budgetMs; waited += 10) {
    if (done()) return;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

function errorRow(host: HTMLElement): HTMLElement | null {
  return host.querySelector('.form-error-row');
}

function retryButton(host: HTMLElement): HTMLButtonElement | null {
  return errorRow(host)?.querySelector('button') ?? null;
}

function eventTypeInput(host: HTMLElement): HTMLInputElement {
  const el = host.querySelector<HTMLInputElement>('.trigger-subscription-row .dropdown-input');
  if (!el) throw new Error('the event-type field is not rendered');
  return el;
}

/** The labels the picker currently offers. Focus is what opens a `freeText`
 *  dropdown, and the menu is portaled, so read it off the document. */
async function openOptionLabels(host: HTMLElement): Promise<string[]> {
  const input = eventTypeInput(host);
  input.blur();
  input.focus();
  await waitFor(() => document.querySelector('.dropdown-option') !== null, 200);
  return Array.from(document.querySelectorAll('.dropdown-option'))
    .filter(el => !el.classList.contains('dropdown-no-results'))
    .map(el => el.textContent ?? '');
}

describe('the trigger event-type picker recovers from a transport failure', () => {
  let host: HTMLElement;
  let unmount: (() => void) | null;

  beforeEach(() => {
    vi.resetModules();
    fetchEventTypes.mockReset();
    document.body.innerHTML = '';
    host = document.createElement('div');
    document.body.appendChild(host);
    unmount = null;
  });

  afterEach(() => {
    unmount?.();
    document.body.innerHTML = '';
    vi.restoreAllMocks();
  });

  it('names the error and offers a Retry control', async () => {
    fetchEventTypes.mockRejectedValue(new TypeError('Load failed'));

    unmount = await mountForm(host);
    await waitFor(() => errorRow(host) !== null);

    expect(errorRow(host)?.textContent).toContain('Load failed');
    expect(retryButton(host)?.textContent).toBe('Retry');
  });

  it('says an event type can still be typed by hand', async () => {
    fetchEventTypes.mockRejectedValue(new TypeError('Load failed'));

    unmount = await mountForm(host);
    await waitFor(() => errorRow(host) !== null);

    expect(errorRow(host)?.textContent).toContain('type an event type by hand');
    expect(eventTypeInput(host).disabled).toBe(false);
  });

  it('offers no empty-valued option while failed, so hand entry survives', async () => {
    fetchEventTypes.mockRejectedValue(new TypeError('Load failed'));

    unmount = await mountForm(host);
    await waitFor(() => errorRow(host) !== null);

    // The reported shape: one option labelled "Failed to load event types"
    // whose value was '', so picking it cleared the field.
    expect(await openOptionLabels(host)).toEqual([]);
    expect(document.querySelector('.dropdown-no-results')).not.toBeNull();
    expect(eventTypeInput(host).value).toBe('DeployFinished');
  });

  it('reloads the list from Retry, with no reopen and no page reload', async () => {
    fetchEventTypes.mockRejectedValue(new TypeError('Load failed'));

    unmount = await mountForm(host);
    await waitFor(() => errorRow(host) !== null);

    fetchEventTypes.mockReset();
    fetchEventTypes.mockResolvedValue(['DeployFinished', 'OuraSleepImported']);
    retryButton(host)?.click();
    await waitFor(() => errorRow(host) === null);

    expect(errorRow(host)).toBeNull();
    expect(await openOptionLabels(host)).toEqual(['DeployFinished', 'OuraSleepImported']);
  });

  it('retries a failed list when the picker is opened', async () => {
    fetchEventTypes.mockRejectedValue(new TypeError('Load failed'));

    unmount = await mountForm(host);
    await waitFor(() => errorRow(host) !== null);

    fetchEventTypes.mockReset();
    fetchEventTypes.mockResolvedValue(['DeployFinished']);
    const input = eventTypeInput(host);
    input.blur();
    input.focus();
    await waitFor(() => errorRow(host) === null);

    // A second open finds the cache loaded, so it must not fetch again.
    expect(await openOptionLabels(host)).toEqual(['DeployFinished']);
    expect(fetchEventTypes).toHaveBeenCalledTimes(1);
  });

  it('still retries when the panel is closed and reopened', async () => {
    fetchEventTypes.mockRejectedValue(new TypeError('Load failed'));

    unmount = await mountForm(host);
    await waitFor(() => errorRow(host) !== null);
    unmount();

    unmount = await mountForm(host);
    await waitFor(() => fetchEventTypes.mock.calls.length === 2);

    expect(fetchEventTypes).toHaveBeenCalledTimes(2);
  });

  it('shares one request across concurrent mounts', async () => {
    // Never settles, so both mounts must join this one request.
    fetchEventTypes.mockImplementation(() => new Promise<string[]>(() => {}));

    const second = document.createElement('div');
    document.body.appendChild(second);
    const unmountFirst = await mountForm(host);
    const unmountSecond = await mountForm(second);
    await waitFor(() => fetchEventTypes.mock.calls.length > 0);
    unmount = () => { unmountFirst(); unmountSecond(); };

    expect(fetchEventTypes).toHaveBeenCalledTimes(1);
  });

  it('does not refetch a list it already loaded', async () => {
    fetchEventTypes.mockResolvedValue(['DeployFinished']);

    unmount = await mountForm(host);
    await waitFor(() => fetchEventTypes.mock.calls.length === 1);
    unmount();

    unmount = await mountForm(host);
    // Nothing to wait FOR here, so give a second fetch time to appear.
    await new Promise((resolve) => setTimeout(resolve, 50));

    expect(fetchEventTypes).toHaveBeenCalledTimes(1);
  });
});
