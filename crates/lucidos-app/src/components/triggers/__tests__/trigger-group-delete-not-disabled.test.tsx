// @vitest-environment jsdom
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

  it('draws no disabled control anywhere on the row', () => {
    render(<TriggerGroupHeader group={group({ member_count: 1 })} />, host);
    expect(disabledControls()).toEqual([]);
  });
});
