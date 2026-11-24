// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

vi.mock('../../../api/client', async () => {
  const actual = await vi.importActual<Record<string, unknown>>('../../../api/client');
  return { ...actual, fetchEventTypes: vi.fn(async () => ['DeployFinished']) };
});

import { TriggerDetails } from '../TriggerDetails';
import { triggers, triggerGroups, panelOverlay, chatModels } from '../../../store/store';
import type { TriggerInfo, TriggerGroup } from '../../../store/types';

import { stubIntentFieldObservers } from './intentFieldStubs';

stubIntentFieldObservers();

const TRIGGER_ID = '220e6c0d-f626-41c3-955c-6b6a66674ce6';
const NIGHTLY = 'group-nightly';
const MACHINE = 'group-machine';

function group(id: string, name: string, order: number): TriggerGroup {
  return { id, name, order, created: '2026-01-01T00:00:00Z', member_count: 0 };
}

function trigger(over: Partial<TriggerInfo> = {}): TriggerInfo {
  return {
    id: TRIGGER_ID,
    name: 'Scheduled CI result',
    slug: 'scheduled-ci-result',
    cron_expressions: ['0 0 9 * * *'],
    timezone: 'UTC',
    paused: false,
    coding_agent_grant: false,
    run: { type: 'intent', intent: 'check CI' },
    ...over,
  };
}

function landFrame(over: Partial<TriggerInfo>): void {
  triggers.value = { status: 'loaded', data: [trigger(over)] };
}

async function waitFor(done: () => boolean, budgetMs = 1000): Promise<void> {
  for (let waited = 0; waited < budgetMs; waited += 10) {
    if (done()) return;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

describe('the open trigger detail page repaints from the TriggerUpdated frame', () => {
  let host: HTMLElement;

  function groupLabel(): string | undefined {
    return host
      .querySelector('.trigger-group-select .dropdown-sizer > span')
      ?.textContent ?? undefined;
  }

  function nameInput(): HTMLInputElement {
    const el = host.querySelector<HTMLInputElement>('.form-group input[type="text"]');
    if (!el) throw new Error('the trigger name field is not rendered');
    return el;
  }

  beforeEach(() => {
    document.body.innerHTML = '';
    host = document.createElement('div');
    document.body.appendChild(host);
    chatModels.value = { status: 'loaded', data: [] };
    triggerGroups.value = {
      status: 'loaded',
      data: [
        group(NIGHTLY, 'Nightly CI & Learning', 0),
        group(MACHINE, 'Machine & Tooling Health', 1),
      ],
    };
    triggers.value = { status: 'loaded', data: [trigger()] };
    panelOverlay.value = { type: 'form', form: { type: 'trigger', triggerId: TRIGGER_ID } };
  });

  afterEach(() => {
    render(null, host);
    document.body.innerHTML = '';
    panelOverlay.value = null;
    triggers.value = { status: 'not-loaded' };
    triggerGroups.value = { status: 'not-loaded' };
    chatModels.value = { status: 'not-loaded' };
    vi.restoreAllMocks();
  });

  it('moves the group picker when the frame assigns a group', async () => {
    render(<TriggerDetails />, host);
    expect(groupLabel()).toBe('(No group)');

    landFrame({ group_id: NIGHTLY });
    await waitFor(() => groupLabel() === 'Nightly CI & Learning');

    expect(groupLabel()).toBe('Nightly CI & Learning');
  });

  it('follows a second frame, so the page never latches on one value', async () => {
    render(<TriggerDetails />, host);

    landFrame({ group_id: NIGHTLY });
    await waitFor(() => groupLabel() === 'Nightly CI & Learning');
    landFrame({ group_id: MACHINE });
    await waitFor(() => groupLabel() === 'Machine & Tooling Health');

    expect(groupLabel()).toBe('Machine & Tooling Health');
  });

  it('repaints a field the frame changed and the user never touched', async () => {
    render(<TriggerDetails />, host);
    expect(nameInput().value).toBe('Scheduled CI result');

    landFrame({ name: 'Nightly CI result' });
    await waitFor(() => nameInput().value === 'Nightly CI result');

    expect(nameInput().value).toBe('Nightly CI result');
  });

  it('keeps a field the user typed into, and still moves the untouched ones', async () => {
    render(<TriggerDetails />, host);

    const input = nameInput();
    input.value = 'My own name';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await waitFor(() => nameInput().value === 'My own name');

    landFrame({ name: 'Renamed elsewhere', group_id: MACHINE });
    await waitFor(() => groupLabel() === 'Machine & Tooling Health');

    expect(nameInput().value).toBe('My own name');
    expect(groupLabel()).toBe('Machine & Tooling Health');
  });

  it('re-arms a field the user edited back to the served value', async () => {
    render(<TriggerDetails />, host);

    const input = nameInput();
    input.value = 'Typed then undone';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await waitFor(() => nameInput().value === 'Typed then undone');
    input.value = 'Scheduled CI result';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await waitFor(() => nameInput().value === 'Scheduled CI result');

    landFrame({ name: 'Renamed elsewhere' });
    await waitFor(() => nameInput().value === 'Renamed elsewhere');

    expect(nameInput().value).toBe('Renamed elsewhere');
  });
});
