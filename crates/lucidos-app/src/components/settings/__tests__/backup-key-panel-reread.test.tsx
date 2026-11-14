// @vitest-environment jsdom
/** Settings → System → Backup → Backup key: the panel re-reads the key state
 *  after a write the engine does not announce.
 *
 *  A setup only stores a pending key, and a verify only stamps a time, so no
 *  `BackupRecipientSet` arrives. A user who copies a new key and cancels must
 *  see the pending key waiting, not the stale legacy warning. */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { useState } from 'preact/hooks';
import { act } from 'preact/test-utils';
import { BackupKeyPanel, backupKeyStatusLine } from '../BackupKeyPanel';
import * as apiClient from '../../../api/client';
import type { BackupKeys } from '../../../api/client';

const NONE: BackupKeys = { recipient: null, pending: false, legacy_key: false, secs_since_verified: null };
/** Synthetic keys: shaped like age keys, made up for this test. */
const PUBLIC_KEY = 'age1examplepublickeyforthistestonlyqqqqqqqqqqqqqqqqqqqqqqqqqq';
const SECRET_KEY = 'AGE-SECRET-KEY-1EXAMPLESECRETKEYFORTHISTESTONLYQQQQQQQQQQQQQQQQQQQQQQQQQQ';

let host: HTMLDivElement;

/** Owns the keys the way BackupSection does, so `onKeysChanged` re-renders. */
function Harness({ initial }: { initial: BackupKeys }) {
  const [keys, setKeys] = useState(initial);
  return <BackupKeyPanel keys={keys} onKeysChanged={setKeys} />;
}

function mount(keys: BackupKeys): void {
  act(() => {
    render(<Harness initial={keys} />, host);
  });
}

function button(label: string): HTMLButtonElement {
  const found = [...host.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent === label);
  expect(found, `no "${label}" button`).toBeDefined();
  return found!;
}

/** Clicks, then lets the panel's async action and its re-render finish. */
async function press(label: string): Promise<void> {
  await act(async () => {
    button(label).click();
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

function status(): string {
  return host.querySelector('.backup-key-status')?.textContent ?? '';
}

beforeEach(() => {
  host?.remove();
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('a setup cancelled after the new key was shown', () => {
  it('leaves the legacy warning for the pending key', async () => {
    const legacy = { ...NONE, legacy_key: true };
    const pending = { ...legacy, pending: true };
    vi.spyOn(apiClient, 'getBackupKey').mockResolvedValue({ key: SECRET_KEY });
    vi.spyOn(apiClient, 'startBackupKeySetup').mockResolvedValue(SECRET_KEY);
    vi.spyOn(apiClient, 'getBackupKeys').mockResolvedValue(pending);
    mount(legacy);
    expect(status()).toBe(backupKeyStatusLine(legacy));

    await press('Switch to a new backup key');
    await press('Create a new backup key');
    await press('Cancel');

    expect(status()).toBe(backupKeyStatusLine(pending));
    expect(button('Paste backup key')).toBeDefined();
  });
});

describe('a re-read that fails after a setup', () => {
  it('still shows the new backup key once', async () => {
    vi.spyOn(apiClient, 'startBackupKeySetup').mockResolvedValue(SECRET_KEY);
    vi.spyOn(apiClient, 'getBackupKeys').mockRejectedValue(new Error('offline'));
    mount(NONE);

    await press('Create backup key');

    expect(host.querySelector('.backup-key-value')?.textContent).toBe(SECRET_KEY);
  });
});

describe('a verify that matches', () => {
  it('shows the new check time', async () => {
    const unchecked = { ...NONE, recipient: PUBLIC_KEY };
    const checked = { ...unchecked, secs_since_verified: 0 };
    vi.spyOn(apiClient, 'verifyBackupKey').mockResolvedValue(true);
    vi.spyOn(apiClient, 'getBackupKeys').mockResolvedValue(checked);
    mount(unchecked);

    await press('Verify backup key');
    const input = host.querySelector<HTMLInputElement>('.backup-key-input')!;
    act(() => {
      input.value = SECRET_KEY;
      input.dispatchEvent(new Event('input', { bubbles: true }));
    });
    await press('Check backup key');

    expect(status()).toBe(backupKeyStatusLine(checked));
  });
});
