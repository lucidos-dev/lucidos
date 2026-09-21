// @vitest-environment jsdom
/**
 * The TypeSafe (Jev) row on Settings → Models → Providers.
 *
 * It wears the same frame as every other provider row, so the things worth
 * pinning are the ones its own state function decides: what the switch reads
 * with no key stored, what folds away behind it, and that off touches neither
 * the credential nor Remove.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

vi.mock('../../../store/actions/credentials', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/credentials')>()),
  submitNewCredential: vi.fn(async () => true),
  deleteCredential: vi.fn(async () => {}),
}));

vi.mock('../../../store/actions/preferences', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/preferences')>()),
  setTypeSafeEnabled: vi.fn(async () => {}),
}));

import { TypeSafeJudgmentSettings } from '../TypeSafeJudgmentSettings';
import { deleteCredential } from '../../../store/actions/credentials';
import { setTypeSafeEnabled } from '../../../store/actions/preferences';
import { credentials, preferences } from '../../../store/store';
import type { CredentialInfo } from '../../../store/types';

function typeSafeCred(): CredentialInfo {
  return {
    id: 'c1',
    service_name: 'typesafe',
    base_urls: ['https://api.typesafe.ai/v1'],
    auth_type: 'api_key',
    auth_header: 'Authorization',
    created_at: '2026-09-19T00:00:00Z',
  } as CredentialInfo;
}

describe('TypeSafeJudgmentSettings', () => {
  let host: HTMLElement;

  const header = () => host.querySelector('[data-search-anchor="models:typesafe"]');
  const usageRow = () => host.querySelector('[data-search-anchor="models:typesafe-usage"]');
  const usage = () => usageRow()?.querySelector('.list-row-details')?.textContent ?? '';
  const secretInput = () => host.querySelector<HTMLInputElement>('input[type="password"]');
  const removeButton = () =>
    [...host.querySelectorAll('button')].find((b) => b.textContent === 'Remove') ?? null;
  const toggle = () => host.querySelector<HTMLInputElement>('.toggle-switch input');
  /** Flip the switch and let Preact's queued rerender land. */
  const press = async (next: boolean): Promise<void> => {
    const input = toggle();
    if (!input) throw new Error('the enable switch is not rendered');
    input.checked = next;
    input.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((resolve) => setTimeout(resolve, 0));
  };
  const withKey = () => { credentials.value = { status: 'loaded', data: [typeSafeCred()] }; };
  const switchedOff = (extra: Record<string, string> = {}) => {
    preferences.value = {
      status: 'loaded',
      data: { provider_enabled_typesafe: 'false', ...extra },
    };
  };

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
    preferences.value = { status: 'loaded', data: {} };
    credentials.value = { status: 'loaded', data: [] };
    vi.mocked(setTypeSafeEnabled).mockClear();
    vi.mocked(deleteCredential).mockClear();
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    preferences.value = { status: 'not-loaded' };
    credentials.value = { status: 'not-loaded' };
  });

  /** The correction this row was rebuilt for. A stored credential stands in
   *  for `/health`, so a workspace that saved nothing reads off, exactly as
   *  xAI does. The preference defaulting to on must not light it up. */
  it('reads off and folded with no key stored', () => {
    render(<TypeSafeJudgmentSettings />, host);
    expect(header()).not.toBeNull();
    expect(toggle()?.checked).toBe(false);
    expect(secretInput()).toBeNull();
    expect(usageRow()).toBeNull();
  });

  it('is on and unfolded once a key is stored and nothing switched it off', () => {
    withKey();
    render(<TypeSafeJudgmentSettings />, host);
    expect(toggle()?.checked).toBe(true);
    expect(secretInput()).not.toBeNull();
    expect(header()?.textContent).toContain('configured');
  });

  /** The only way to type a first key. A provider nobody configured has
   *  nothing to switch, so the press reveals the fields and writes nothing. */
  it('reveals the secret field without writing, with no key stored', async () => {
    render(<TypeSafeJudgmentSettings />, host);
    await press(true);
    expect(secretInput()).not.toBeNull();
    expect(setTypeSafeEnabled).not.toHaveBeenCalled();
  });

  it('keeps the secret write-only, never rendering a stored value', () => {
    withKey();
    render(<TypeSafeJudgmentSettings />, host);
    expect(secretInput()?.type).toBe('password');
    expect(secretInput()?.value).toBe('');
  });

  it('draws no switch position before both reads land', () => {
    credentials.value = { status: 'loading' };
    render(<TypeSafeJudgmentSettings />, host);
    expect(header()).not.toBeNull();
    expect(host.querySelector('.toggle-switch-loading')).not.toBeNull();
    expect(toggle()).toBeNull();
    expect(secretInput()).toBeNull();
  });

  /** An unloaded preference read is not "no site on". Saying so would claim
   *  nothing is running on a workspace where both are. The fold is what
   *  guarantees it now, since `open` needs both reads, so this pins the row
   *  away rather than an empty string inside it. */
  it('asserts nothing about usage while the preferences load', () => {
    withKey();
    preferences.value = { status: 'loading' };
    render(<TypeSafeJudgmentSettings />, host);
    expect(usageRow()).toBeNull();
    expect(host.textContent).not.toContain('Nothing yet');
  });

  /** Off parks the provider. Deleting the key is Remove's job, and the two
   *  must stay separable or a user cannot put a key down and pick it up. */
  it('switches off without touching the stored key', async () => {
    withKey();
    render(<TypeSafeJudgmentSettings />, host);
    await press(false);
    expect(setTypeSafeEnabled).toHaveBeenCalledWith(false);
    expect(deleteCredential).not.toHaveBeenCalled();

    switchedOff();
    render(<TypeSafeJudgmentSettings />, host);
    expect(header()?.textContent).toContain('switched off, key kept');
    expect(header()?.textContent).toContain('configured');
    expect(secretInput()).toBeNull();
  });

  it('switches a parked provider back on', async () => {
    withKey();
    switchedOff();
    render(<TypeSafeJudgmentSettings />, host);
    expect(toggle()?.checked).toBe(false);
    await press(true);
    expect(setTypeSafeEnabled).toHaveBeenCalledWith(true);
  });

  /** Remove sits on the header row beside the switch, like every other
   *  provider's, so a parked key can be deleted without switching back on. */
  it('keeps Remove on the header row while switched off', () => {
    withKey();
    switchedOff();
    render(<TypeSafeJudgmentSettings />, host);
    const remove = removeButton();
    expect(remove).not.toBeNull();
    expect(header()?.querySelector('.settings-row-options')?.contains(remove!)).toBe(true);
  });

  it('offers no Remove until there is a key to remove', () => {
    render(<TypeSafeJudgmentSettings />, host);
    expect(removeButton()).toBeNull();
  });

  /** A stored key IS the judge tool's whole condition, so it is on with no
   *  preference set at all (ADR 0223). Neither classification is, which is what
   *  keeps the promise that a key moves nothing on its own. */
  it('names the judge tool on a stored key, with no site switched over', () => {
    withKey();
    render(<TypeSafeJudgmentSettings />, host);
    expect(usage()).toContain('judge tool');
    expect(usage()).not.toContain('Command guard');
    expect(usage()).not.toContain('Query classification');
  });

  it('names each site that is on', () => {
    withKey();
    preferences.value = {
      status: 'loaded',
      data: { judgment_command_guard: 'jev', judgment_query_classification: 'jev' },
    };
    render(<TypeSafeJudgmentSettings />, host);
    expect(usage()).toContain('Command guard');
    expect(usage()).toContain('Query classification');
  });

  it('names only the site that is on', () => {
    withKey();
    preferences.value = { status: 'loaded', data: { judgment_command_guard: 'jev' } };
    render(<TypeSafeJudgmentSettings />, host);
    expect(usage()).toContain('Command guard');
    expect(usage()).not.toContain('Query classification');
  });

  /** The site preferences outlive the master switch, so a usage line left on
   *  screen would name two classifications running on a provider that is off.
   *  Folding it away with the rest is what keeps it honest. */
  it('claims no site is in use while the provider is switched off', () => {
    withKey();
    switchedOff({ judgment_command_guard: 'jev', judgment_query_classification: 'jev' });
    render(<TypeSafeJudgmentSettings />, host);
    expect(usageRow()).toBeNull();
    expect(host.textContent).not.toContain('Command guard');
  });
});
