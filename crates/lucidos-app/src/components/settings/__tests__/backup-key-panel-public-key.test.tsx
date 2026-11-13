// @vitest-environment jsdom
/** Settings → System → Backup → Backup key: the public key.
 *
 *  The panel shows the workspace's public key, so the user can set another
 *  workspace up with it. The own-key step takes only a public key: a pasted
 *  backup key is refused in the browser and never sent.
 *
 *  Plan: `docs/plans/2026-10-21-reuse-an-existing-age-backup-key.md`. */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { BackupKeyPanel, publicKeyPasteProblem } from '../BackupKeyPanel';
import * as apiClient from '../../../api/client';
import type { BackupKeys } from '../../../api/client';

const NONE: BackupKeys = { recipient: null, pending: false, legacy_key: false, secs_since_verified: null };
/** Synthetic keys: shaped like age keys, made up for this test. */
const PUBLIC_KEY = 'age1examplepublickeyforthistestonlyqqqqqqqqqqqqqqqqqqqqqqqqqq';
const SECRET_KEY = 'AGE-SECRET-KEY-1EXAMPLESECRETKEYFORTHISTESTONLYQQQQQQQQQQQQQQQQQQQQQQQQQQ';

let host: HTMLDivElement;

function mount(keys: BackupKeys): void {
  act(() => {
    render(<BackupKeyPanel keys={keys} onKeysChanged={() => {}} />, host);
  });
}

function button(label: string): HTMLButtonElement {
  const found = [...host.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent === label);
  expect(found, `no "${label}" button`).toBeDefined();
  return found!;
}

function field(): HTMLInputElement {
  const input = host.querySelector<HTMLInputElement>('.backup-key-input');
  expect(input, 'no paste field').not.toBeNull();
  return input!;
}

async function pasteIntoOwnStep(text: string): Promise<void> {
  act(() => button('Use my own age public key').click());
  const input = field();
  act(() => {
    input.value = text;
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await act(async () => button('Use this key').click());
}

beforeEach(() => {
  host?.remove();
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('the public key', () => {
  it('is shown with Copy once a recipient seals backups', () => {
    mount({ ...NONE, recipient: PUBLIC_KEY });
    expect(host.querySelector('.backup-key-value')?.textContent).toBe(PUBLIC_KEY);
    expect(host.querySelector('[aria-label="Copy public key"]')).not.toBeNull();
  });

  it('is absent before setup and while only a minted key waits', () => {
    mount(NONE);
    expect(host.querySelector('.backup-key-value')).toBeNull();
    mount({ ...NONE, pending: true });
    expect(host.querySelector('.backup-key-value')).toBeNull();
  });
});

describe('the own-key step', () => {
  it('says where to find the public key', () => {
    mount(NONE);
    act(() => button('Use my own age public key').click());
    const note = host.querySelector('.backup-key-step .backup-key-note')?.textContent ?? '';
    expect(note).toContain('age1');
    expect(note).toContain('Settings, System, Backup');
    expect(note).toContain('age-keygen -y');
  });

  it('refuses a pasted backup key without sending it, and clears the field', async () => {
    const send = vi.spyOn(apiClient, 'setOwnBackupRecipient');
    mount(NONE);
    await pasteIntoOwnStep(`  ${SECRET_KEY.toLowerCase()}\n`);
    expect(send).not.toHaveBeenCalled();
    expect(field().value).toBe('');
  });

  it('sends a public key', async () => {
    const send = vi.spyOn(apiClient, 'setOwnBackupRecipient').mockResolvedValue({ ...NONE, recipient: PUBLIC_KEY });
    mount(NONE);
    await pasteIntoOwnStep(` ${PUBLIC_KEY} `);
    expect(send).toHaveBeenCalledWith(PUBLIC_KEY);
  });
});

describe('publicKeyPasteProblem', () => {
  it('names the public key for a backup key of any case, and never repeats it', () => {
    // A key file pasted into a one-line field loses its line breaks, so the
    // secret key comes after the comments.
    const keyFile = `# created: 2026-01-01T00:00:00Z# public key: ${PUBLIC_KEY}${SECRET_KEY}`;
    for (const pasted of [SECRET_KEY, SECRET_KEY.toLowerCase(), `\n${SECRET_KEY} `, keyFile]) {
      const problem = publicKeyPasteProblem(pasted);
      expect(problem).toContain('age1');
      expect(problem).not.toContain(SECRET_KEY.slice(16));
    }
  });

  it('leaves a public key to the engine', () => {
    expect(publicKeyPasteProblem(PUBLIC_KEY)).toBeNull();
  });
});
