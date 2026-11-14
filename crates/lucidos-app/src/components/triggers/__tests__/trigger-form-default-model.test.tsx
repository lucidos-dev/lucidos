// @vitest-environment jsdom
/**
 * An unpinned trigger runs on the account's trigger model, so the form's
 * Default row has to name that model and never the chat model.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

vi.mock('../../../api/client', async () => {
  const actual = await vi.importActual<Record<string, unknown>>('../../../api/client');
  return {
    ...actual,
    fetchEventTypes: vi.fn(async () => []),
    listTriggers: vi.fn(async () => []),
    listHistoricalTriggers: vi.fn(async () => []),
  };
});

import { TriggerDetails } from '../TriggerDetails';
import { triggerGroups, panelOverlay, chatModels, triggers, preferences } from '../../../store/store';

import { stubIntentFieldObservers } from './intentFieldStubs';

stubIntentFieldObservers();

describe('the trigger form Default row', () => {
  let host: HTMLElement;

  beforeEach(() => {
    document.body.innerHTML = '';
    host = document.createElement('div');
    document.body.appendChild(host);
    chatModels.value = { status: 'loaded', data: [] };
    triggerGroups.value = { status: 'loaded', data: [] };
    panelOverlay.value = { type: 'form', form: { type: 'trigger' } };
    preferences.value = {
      status: 'loaded',
      data: { chat_model: 'model-chat', trigger_model: 'model-trigger' },
    };
  });

  afterEach(() => {
    render(null, host);
    panelOverlay.value = null;
    triggers.value = { status: 'not-loaded' };
    triggerGroups.value = { status: 'not-loaded' };
    chatModels.value = { status: 'not-loaded' };
    preferences.value = { status: 'not-loaded' };
  });

  it('names the trigger model, not the chat model', () => {
    render(<TriggerDetails />, host);

    const text = host.textContent ?? '';
    expect(text).toContain('Default (model-trigger');
    expect(text).not.toContain('Default (model-chat');
  });
});
