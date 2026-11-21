// @vitest-environment jsdom
/** Settings → Keyboard Shortcuts → Composer: the Enter key switch.
 *
 *  It shows the stored mode, and a press writes `composer_enter_key`.
 *
 *  Plan: `docs/plans/2026-10-16-composer-enter-key-setting.md`. */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { COMPOSER_ENTER_KEY_LABELS, KeyboardShortcutsSection } from '../KeyboardShortcutsSection';
import { preferences } from '../../../store/store';
import * as apiClient from '../../../api/client';

let host: HTMLDivElement;

function mount(stored?: string): void {
  preferences.value = { status: 'loaded', data: stored ? { composer_enter_key: stored } : {} };
  act(() => {
    render(<KeyboardShortcutsSection />, host);
  });
}

function buttons(): HTMLButtonElement[] {
  const row = host.querySelector('[data-search-anchor="keyboard-shortcuts:composer-enter-key"]');
  expect(row, 'no Enter key row').not.toBeNull();
  return [...row!.querySelectorAll<HTMLButtonElement>('.segmented-btn')];
}

function pressed(): string[] {
  return buttons().filter((b) => b.getAttribute('aria-pressed') === 'true').map((b) => b.textContent ?? '');
}

beforeEach(() => {
  localStorage.clear();
  host?.remove();
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('the Enter key row', () => {
  it('offers both modes, and Sends is on when nothing is stored', () => {
    mount();
    expect(buttons().map((b) => b.textContent)).toEqual([COMPOSER_ENTER_KEY_LABELS.send, COMPOSER_ENTER_KEY_LABELS.newline]);
    expect(pressed()).toEqual([COMPOSER_ENTER_KEY_LABELS.send]);
  });

  it('shows the stored mode', () => {
    mount('newline');
    expect(pressed()).toEqual([COMPOSER_ENTER_KEY_LABELS.newline]);
  });

  it('writes the preference on a press', async () => {
    const save = vi.spyOn(apiClient, 'setPreference').mockResolvedValue({ success: true } as Awaited<ReturnType<typeof apiClient.setPreference>>);
    mount();
    const newline = buttons().find((b) => b.textContent === COMPOSER_ENTER_KEY_LABELS.newline)!;
    await act(async () => newline.click());
    await vi.waitFor(() => expect(save).toHaveBeenCalled());
    expect(save.mock.calls[0].slice(0, 2)).toEqual(['composer_enter_key', 'newline']);
    expect(pressed()).toEqual([COMPOSER_ENTER_KEY_LABELS.newline]);
  });
});
