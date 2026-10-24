// @vitest-environment jsdom
/**
 * Diff and the standing apply, merged into one box on a row too narrow for
 * both. The menu's rows are the members' own rows, so each action stays
 * reachable and the armed state stays visible.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { DiffApplyMenu } from '../WaitingBanner';
import { DiffIcon } from '../../shared/icons';
import type { HeaderActionSpec } from '../../layout/headerActions';

const openDiff = vi.fn();
const toggleApply = vi.fn();

const members = (armed: boolean): HeaderActionSpec[] => [
  { key: 'thread-diff', dataRole: 'thread-diff', label: 'Diff', icon: () => <DiffIcon />, onClick: openDiff },
  { key: 'standing-apply', dataRole: 'standing-apply', label: 'Apply when settled', active: armed, icon: () => <DiffIcon />, onClick: toggleApply },
];

const ATTRS = { 'data-row-item': 'fold', 'data-fold-key': 'diff-and-standing-apply' };
let host: HTMLDivElement;

async function draw(armed: boolean): Promise<void> {
  await act(() => { render(<DiffApplyMenu members={members(armed)} attrs={ATTRS} />, host); });
}

const face = () => host.querySelector<HTMLButtonElement>('button[data-role="diff-apply-menu"]')!;
const row = (role: string) => document.querySelector<HTMLButtonElement>(`.thread-overflow-menu [data-role="${role}"]`);

async function openMenu(): Promise<void> {
  await act(() => { face().dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, detail: 1 })); });
}

beforeEach(() => {
  openDiff.mockClear();
  toggleApply.mockClear();
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  document.querySelectorAll('.thread-overflow-menu').forEach((el) => el.remove());
});

describe('the merged Diff and standing apply control', () => {
  it('is one icon box carrying the row measurement attributes', async () => {
    await draw(false);
    expect(host.querySelectorAll('button')).toHaveLength(1);
    expect(face().getAttribute('data-row-item')).toBe('fold');
    expect(face().getAttribute('data-fold-key')).toBe('diff-and-standing-apply');
    expect(face().getAttribute('aria-haspopup')).toBe('menu');
    expect(face().textContent).toBe('');
  });

  it('wears the armed tone only while the standing apply is armed', async () => {
    await draw(false);
    expect(face().hasAttribute('aria-pressed')).toBe(false);
    await draw(true);
    expect(face().getAttribute('data-tone')).toBe('armed');
    expect(face().getAttribute('aria-pressed')).toBe('true');
  });

  it('runs Diff from its menu row', async () => {
    await draw(true);
    await openMenu();
    row('thread-diff')!.click();
    expect(openDiff).toHaveBeenCalledTimes(1);
  });

  it('arms and disarms the standing apply from its menu row', async () => {
    await draw(true);
    await openMenu();
    const apply = row('standing-apply')!;
    expect(apply.getAttribute('role')).toBe('menuitemcheckbox');
    expect(apply.getAttribute('aria-checked')).toBe('true');
    apply.click();
    expect(toggleApply).toHaveBeenCalledTimes(1);
  });
});
