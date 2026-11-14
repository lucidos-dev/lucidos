// @vitest-environment jsdom
/**
 * **Invariant: the group header's delete never renders disabled.**
 *
 * ADR 0168, the same rule `changes/__tests__/no-disabled-change-action.test.tsx`
 * holds for the change actions. `.icon-btn:disabled` sets `pointer-events:
 * none`, so a disabled button answers neither hover nor long press, and the
 * tooltip stating the block is the one thing it cannot show.
 *
 * What shipped: a group with members drew the trash icon at 25% opacity, with
 * `data-tooltip="Move triggers out first"` on that dead control and nothing
 * else on the row saying so. The control read as broken. The button stays live
 * instead, and `deleteTriggerGroup` toasts the server's refusal.
 *
 * Rendered rather than asserted through a selector: what is banned is a
 * `disabled` attribute in the markup.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';

import { TriggerGroupHeader } from '../TriggerGroupHeader';
import type { TriggerGroup } from '../../../store/types';

function group(over: Partial<TriggerGroup> = {}): TriggerGroup {
  return {
    id: '8b1f0a2c-0d64-4a1e-9f3b-2c5d7e8a9b01',
    name: 'Nightly',
    order: 0,
    created: '2026-01-01T00:00:00Z',
    member_count: 0,
    ...over,
  };
}

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

function deleteButton(): HTMLButtonElement {
  const btn = host.querySelector<HTMLButtonElement>('button.trigger-group-delete');
  if (!btn) throw new Error('the group header draws no delete button');
  return btn;
}

function disabledControls(): string[] {
  return [...host.querySelectorAll('button')]
    .filter((b) => (b as HTMLButtonElement).disabled)
    .map((b) => b.getAttribute('aria-label') ?? b.className);
}

describe('the trigger group header', () => {
  it('draws a live delete for a group that still holds triggers', () => {
    render(<TriggerGroupHeader group={group({ member_count: 3 })} />, host);
    expect(deleteButton().disabled).toBe(false);
  });

  it('puts the reason on a control that can answer a hover or a long press', () => {
    render(<TriggerGroupHeader group={group({ member_count: 3 })} />, host);
    expect(deleteButton().getAttribute('data-tooltip')).toBe('Move triggers out first');
  });

  it('says plainly what an empty group offers', () => {
    render(<TriggerGroupHeader group={group()} />, host);
    expect(deleteButton().disabled).toBe(false);
    expect(deleteButton().getAttribute('data-tooltip')).toBe('Delete group');
  });

  // The whole row, not just the trash: rename is in the same class and would
  // take its tooltip out of reach the same way.
  it('draws no disabled control anywhere on the row', () => {
    render(<TriggerGroupHeader group={group({ member_count: 1 })} />, host);
    expect(disabledControls()).toEqual([]);
  });
});
