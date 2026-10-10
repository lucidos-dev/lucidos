// @vitest-environment jsdom
/**
 * The trigger form's "Send directly to Archive" toggle is `go_to_review`
 * inverted (ADR 0349). The stored field keeps its meaning, so an existing
 * trigger keeps its behaviour: on means `go_to_review` is false.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

vi.mock('../../../api/client', async () => {
  const actual = await vi.importActual<Record<string, unknown>>('../../../api/client');
  return { ...actual, fetchEventTypes: vi.fn(async () => []) };
});

import { TriggerDetails } from '../TriggerDetails';
import { triggers, triggerGroups, panelOverlay, chatModels } from '../../../store/store';
import type { TriggerInfo } from '../../../store/types';

import { stubIntentFieldObservers } from './intentFieldStubs';

stubIntentFieldObservers();

const TRIGGER_ID = '5b0e0f7e-3a5c-4f0b-9a49-6f3f1d2a7c11';

function trigger(goToReview: boolean): TriggerInfo {
  return {
    id: TRIGGER_ID,
    name: 'Daily digest',
    cron_expressions: ['0 0 9 * * *'],
    timezone: 'UTC',
    paused: false,
    run: { type: 'intent', intent: 'summarize the day' },
    go_to_review: goToReview,
  };
}

describe('the Send directly to Archive toggle', () => {
  let host: HTMLElement;

  const toggle = () => {
    const el = host.querySelector<HTMLInputElement>('[data-role="send-directly-to-archive"]');
    if (!el) throw new Error('the toggle is not rendered');
    return el;
  };

  beforeEach(() => {
    document.body.innerHTML = '';
    host = document.createElement('div');
    document.body.appendChild(host);
    chatModels.value = { status: 'loaded', data: [] };
    triggerGroups.value = { status: 'loaded', data: [] };
    panelOverlay.value = { type: 'form', form: { type: 'trigger', triggerId: TRIGGER_ID } };
  });

  afterEach(() => {
    render(null, host);
    panelOverlay.value = null;
    triggers.value = { status: 'not-loaded' };
    triggerGroups.value = { status: 'not-loaded' };
    chatModels.value = { status: 'not-loaded' };
    vi.restoreAllMocks();
  });

  it('is on for the default, a trigger that does not go to review', () => {
    triggers.value = { status: 'loaded', data: [trigger(false)] };
    render(<TriggerDetails />, host);
    expect(toggle().checked).toBe(true);
    expect(host.textContent).not.toContain('Send to Review');
  });

  it('is off for a trigger whose runs stay in Current', () => {
    triggers.value = { status: 'loaded', data: [trigger(true)] };
    render(<TriggerDetails />, host);
    expect(toggle().checked).toBe(false);
  });
});
