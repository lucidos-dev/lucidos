// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

const { updateTrigger } = vi.hoisted(() => ({
  updateTrigger: vi.fn(async () => ({ success: true })),
}));

vi.mock('../../../api/client', async () => {
  const actual = await vi.importActual<Record<string, unknown>>('../../../api/client');
  return {
    ...actual,
    fetchEventTypes: vi.fn(async () => []),
    listTriggers: vi.fn(async () => ({ triggers: [] })),
    updateTrigger,
  };
});

import { TriggerDetails } from '../TriggerDetails';
import { triggers, triggerGroups, panelOverlay, chatModels } from '../../../store/store';
import type { TriggerInfo } from '../../../store/types';

import { stubIntentFieldObservers } from './intentFieldStubs';

stubIntentFieldObservers();

const TRIGGER_ID = '9d2c4a61-7b3e-4f1a-8c55-2e6b0f4d9a18';

function trigger(run: TriggerInfo['run']): TriggerInfo {
  return {
    id: TRIGGER_ID,
    name: 'Weekly report',
    slug: 'weekly-report',
    cron_expressions: ['0 0 9 * * 1'],
    timezone: 'UTC',
    paused: false,
    coding_agent_grant: false,
    run,
    program_grants: [{ pattern: 'Bash(curl:*)', migrated_invocation: false }],
    folder_grants: [{ folder: '/Users/me/Reports' }, { folder: '/Users/me/Exports' }],
  };
}

describe('folder grants in Trigger details', () => {
  let host: HTMLElement;

  const folderRows = () => [...host.querySelectorAll('[data-role="folder-grants"] .allowlist-row')];

  beforeEach(() => {
    document.body.innerHTML = '';
    host = document.createElement('div');
    document.body.appendChild(host);
    chatModels.value = { status: 'loaded', data: [] };
    triggerGroups.value = { status: 'loaded', data: [] };
    panelOverlay.value = { type: 'form', form: { type: 'trigger', triggerId: TRIGGER_ID } };
    updateTrigger.mockClear();
  });

  afterEach(() => {
    render(null, host);
    panelOverlay.value = null;
    triggers.value = { status: 'not-loaded' };
    triggerGroups.value = { status: 'not-loaded' };
    chatModels.value = { status: 'not-loaded' };
  });

  it('lists every folder grant for a script trigger too', () => {
    triggers.value = { status: 'loaded', data: [trigger({ type: 'script', path: 'triggers/report/run.sh' })] };
    render(<TriggerDetails />, host);
    const folders = folderRows().map((row) => row.querySelector('input')?.value);
    expect(folders).toEqual(['/Users/me/Reports', '/Users/me/Exports']);
  });

  it('revokes only the folder whose row was deleted', async () => {
    triggers.value = { status: 'loaded', data: [trigger({ type: 'intent', intent: 'write the report' })] };
    render(<TriggerDetails />, host);
    folderRows()[1].querySelector<HTMLButtonElement>('button')!.click();
    await vi.waitFor(() => expect(updateTrigger).toHaveBeenCalledTimes(1));
    expect(updateTrigger).toHaveBeenCalledWith(TRIGGER_ID, {
      revoke_folder_grants: ['/Users/me/Exports'],
    });
  });

  it('revokes a program grant on its own', async () => {
    triggers.value = { status: 'loaded', data: [trigger({ type: 'intent', intent: 'write the report' })] };
    render(<TriggerDetails />, host);
    host.querySelector<HTMLButtonElement>('button[aria-label="Delete pattern Bash(curl:*)"]')!.click();
    await vi.waitFor(() => expect(updateTrigger).toHaveBeenCalledTimes(1));
    expect(updateTrigger).toHaveBeenCalledWith(TRIGGER_ID, {
      revoke_program_grants: ['Bash(curl:*)'],
    });
  });

  it('says so when the trigger holds no folder grant', () => {
    triggers.value = {
      status: 'loaded',
      data: [{ ...trigger({ type: 'intent', intent: 'write the report' }), folder_grants: [] }],
    };
    render(<TriggerDetails />, host);
    expect(host.querySelector('[data-role="folder-grants"]')?.textContent).toBe('No folder grants.');
  });
});

describe('denies in Trigger details', () => {
  let host: HTMLElement;

  const denyRows = () => [...host.querySelectorAll('[data-role="trigger-denies"] .allowlist-row')];

  beforeEach(() => {
    document.body.innerHTML = '';
    host = document.createElement('div');
    document.body.appendChild(host);
    chatModels.value = { status: 'loaded', data: [] };
    triggerGroups.value = { status: 'loaded', data: [] };
    panelOverlay.value = { type: 'form', form: { type: 'trigger', triggerId: TRIGGER_ID } };
    updateTrigger.mockClear();
  });

  afterEach(() => {
    render(null, host);
    panelOverlay.value = null;
    triggers.value = { status: 'not-loaded' };
    triggerGroups.value = { status: 'not-loaded' };
    chatModels.value = { status: 'not-loaded' };
  });

  const denied = (): TriggerInfo => ({
    ...trigger({ type: 'script', path: 'triggers/report/run.sh' }),
    program_denies: ['Bash(gh:*)'],
    folder_denies: ['/Users/me/Private'],
  });

  it('lists every deny, programs then folders, for a script trigger too', () => {
    triggers.value = { status: 'loaded', data: [denied()] };
    render(<TriggerDetails />, host);
    expect(denyRows().map((row) => row.querySelector('input')?.value)).toEqual([
      'Bash(gh:*)',
      '/Users/me/Private',
    ]);
  });

  it('says so when nothing is denied', () => {
    triggers.value = { status: 'loaded', data: [trigger({ type: 'intent', intent: 'write the report' })] };
    render(<TriggerDetails />, host);
    expect(host.querySelector('[data-role="trigger-denies"]')?.textContent).toContain('Nothing denied.');
  });

  it('lifts only the deny whose row was deleted', async () => {
    triggers.value = { status: 'loaded', data: [denied()] };
    render(<TriggerDetails />, host);
    denyRows()[1].querySelector<HTMLButtonElement>('button')!.click();
    await vi.waitFor(() => expect(updateTrigger).toHaveBeenCalledTimes(1));
    expect(updateTrigger).toHaveBeenCalledWith(TRIGGER_ID, { revoke_folder_denies: ['/Users/me/Private'] });
    denyRows()[0].querySelector<HTMLButtonElement>('button')!.click();
    await vi.waitFor(() => expect(updateTrigger).toHaveBeenCalledTimes(2));
    expect(updateTrigger).toHaveBeenLastCalledWith(TRIGGER_ID, { revoke_program_denies: ['Bash(gh:*)'] });
  });
});
