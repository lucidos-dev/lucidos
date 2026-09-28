// @vitest-environment jsdom
// The Ungrouped section collapses like a real group. Its heading only renders
// beside real groups. With none left, a saved collapse must not hide rows that
// no heading can reopen.
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { TriggersView } from '../TriggersView';
import {
  triggers,
  triggerGroups,
  collapsedTriggerSectionIds,
  UNGROUPED_TRIGGER_SECTION_ID,
} from '../../../store/store';
import type { TriggerInfo } from '../../../store/types';

const LOOSE: TriggerInfo = {
  id: 'loose',
  name: 'Morning digest',
  cron_expressions: ['0 9 * * *'],
  timezone: 'UTC',
  paused: false,
  run: { type: 'intent', intent: 'send the digest' },
};

const CI_GROUP = { id: 'ci', name: 'CI', order: 0, created: '2026-01-01T00:00:00Z', member_count: 0 };

async function waitFor(done: () => boolean, budgetMs = 1000): Promise<void> {
  for (let waited = 0; waited < budgetMs; waited += 20) {
    if (done()) return;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

describe('the Ungrouped section', () => {
  let host: HTMLElement;
  const row = () => host.querySelector('.trigger-row[data-trigger-id="loose"]');
  const toggle = () =>
    host.querySelector<HTMLButtonElement>('.trigger-group-header-ungrouped .trigger-group-toggle');

  beforeEach(() => {
    localStorage.clear();
    host = document.createElement('div');
    document.body.appendChild(host);
    triggers.value = { status: 'loaded', data: [LOOSE] };
    collapsedTriggerSectionIds.value = new Set();
  });

  afterEach(() => {
    render(null, host);
    host.remove();
  });

  it('collapses and reopens from its heading', async () => {
    triggerGroups.value = { status: 'loaded', data: [CI_GROUP] };
    render(<TriggersView />, host);
    expect(toggle()?.getAttribute('aria-expanded')).toBe('true');
    expect(row()).not.toBeNull();

    toggle()!.click();
    await waitFor(() => row() === null);
    expect(row()).toBeNull();
    expect(toggle()?.getAttribute('aria-expanded')).toBe('false');
    expect(collapsedTriggerSectionIds.value.has(UNGROUPED_TRIGGER_SECTION_ID)).toBe(true);

    toggle()!.click();
    await waitFor(() => row() !== null);
    expect(row()).not.toBeNull();
    expect(collapsedTriggerSectionIds.value.has(UNGROUPED_TRIGGER_SECTION_ID)).toBe(false);
  });

  it('shows its rows when no groups exist, even if it was left collapsed', () => {
    triggerGroups.value = { status: 'loaded', data: [] };
    collapsedTriggerSectionIds.value = new Set([UNGROUPED_TRIGGER_SECTION_ID]);
    render(<TriggersView />, host);

    expect(toggle()).toBeNull();
    expect(row()).not.toBeNull();
  });
});
