/**
 * Backup setup spans two Settings pages, and the Backup page owns only one of
 * them: the provider account is connected under Settings → Accounts, and this
 * page has no account UI at all. For a long time it said so in prose ("Connect
 * your X account in Settings → Accounts"), which leaves the user to walk the
 * route themselves, and is a route a 2026-08-05 session got wrong by naming the
 * Backup page instead. It must be an affordance, not a path.
 *
 * The same page also gained a hand-off to the agent, which can do the connecting
 * itself and knows the parts the page cannot enforce (saving the encryption key).
 *
 * Source-scan rather than a mounted render, for the same reason as
 * `external-links-row-reachable.test.ts`: `BackupSection` pulls in the backup
 * API, OAuth state and live SSE progress, and what is being pinned here is which
 * action each affordance is wired to, not the mechanism that renders it.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));

/** Block comments stripped, so an assertion about what the UI SAYS can't be
 *  satisfied (or tripped) by a comment that happens to discuss it. Covers both
 *  JSX `{/* … *\/}` and ordinary block comments. */
function code(path: string): string {
  return readFileSync(resolve(here, path), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
}

const source = code('../BackupSection.tsx');
const menuSource = code('../../../store/actions/menu.ts');

describe('the not-connected state offers a way out, not a path to walk', () => {
  it('wires a button to the Connected accounts deep-link', () => {
    expect(source).toContain('openConnectedAccountsSettings');
    // A <button onClick>, so it is clickable and keyboard-reachable. Prose
    // naming the path is what this replaced.
    expect(source).toMatch(/onClick=\{openConnectedAccountsSettings\}/);
  });

  it('no longer instructs the user to navigate there themselves', () => {
    expect(source).not.toMatch(/account in Settings . Accounts/);
  });

  // The deep-link has to land on the SECTION, not just the page: Accounts has
  // two sections and the one being pointed at is the accounts list.
  it('the deep-link scrolls to the Connected accounts section', () => {
    expect(menuSource).toContain('openConnectedAccountsSettings');
    expect(menuSource).toContain("settingsScrollTarget.value = 'accounts:connected'");
    expect(menuSource).toContain("settingsSubview.value = 'accounts'");
  });
});

describe('the chat hand-off', () => {
  it('opens a new chat with a setup prompt', () => {
    expect(source).toContain('askLucidosToSetUpBackups');
    expect(source).toMatch(/target: 'new-chat'/);
  });

  // The prompt must name the parts the page cannot do for the user, or the
  // hand-off just re-opens the same question in a different pane.
  it('names connecting the account and saving the key', () => {
    const prompt = source.slice(
      source.indexOf('askLucidosToSetUpBackups'),
      source.indexOf('askLucidosToSetUpBackups') + 900,
    );
    expect(prompt).toContain('connect the account');
    expect(prompt).toContain('encryption key');
  });

  // Routed through the shared navigation entry point so it clears the settings
  // overlay, allocates a fresh draft and focuses the prompt like every other
  // new-chat path. Poking compose directly would skip all three.
  it('routes through handleNavigationRequest', () => {
    expect(source).toContain('handleNavigationRequest({');
  });
});

/**
 * The page must address the provider the workspace is CONFIGURED for. It used
 * to seed the dropdown with the first registry entry, which is always Google
 * Drive, so an install configured for Dropbox pointed its health card, its
 * connected / ready verdict, its Grant access button and its Back up now button
 * at a provider the user never chose.
 *
 * Same source-scan rationale as above: what is pinned is the wiring, and the
 * decision itself is unit-tested as a pure function in
 * `backup-provider-scopes.test.ts`.
 */
describe('the provider dropdown reflects and writes the configured destination', () => {
  it('seeds through the shared decision instead of the first registry entry', () => {
    expect(source).toContain('pickInitialProvider(');
    // The exact shape of the old bug. `p[0]` as the unconditional seed is what
    // overrode `backup_provider`.
    expect(source).not.toMatch(/setSelectedProvider\(\s*p\[0\]\.id\s*\)/);
  });

  it('settles both requests before selecting, so the fallback cannot win a race', () => {
    // Seeding from whichever of the two fetches resolved first would reproduce
    // the bug intermittently, and would also flip the visible dropdown from one
    // provider to another mid-load.
    expect(source).toContain('Promise.allSettled([');
    expect(source).toContain('getBackupProviders()');
    expect(source).toContain('getBackupSchedule()');
  });

  it('persists a pick instead of holding it as view-only state', () => {
    expect(source).toMatch(/onChange=\{\(v\) => void handleProviderChange\(v\)\}/);
    expect(source).toContain('async function handleProviderChange');
    // The bare setter as the whole handler is what left the page and the
    // preference free to disagree.
    expect(source).not.toMatch(/onChange=\{\(v\) => setSelectedProvider\(v\)\}/);
  });

  it('sends the current schedule with the new provider', () => {
    // One endpoint writes both keys, so omitting the schedule here would turn a
    // destination change into a silent schedule change.
    expect(source).toContain('setBackupSchedule(newProvider, schedule)');
  });

  it('re-reads the ready verdict after switching, since it is per provider', () => {
    const handler = source.slice(
      source.indexOf('async function handleProviderChange'),
      source.indexOf('async function handleScheduleChange'),
    );
    expect(handler).toContain('getBackupProviders()');
  });

  it('rolls back and toasts when the write is refused', () => {
    const handler = source.slice(
      source.indexOf('async function handleProviderChange'),
      source.indexOf('async function handleScheduleChange'),
    );
    expect(handler).toContain('setSelectedProvider(previous)');
    expect(handler).toMatch(/showToast\(`Failed to set backup provider/);
  });

  it('does not roll back when only the follow-up refresh fails', () => {
    // The write and the refresh need SEPARATE catches. Under one, a refresh
    // failure after a persisted write rolls the dropdown back to a provider
    // the engine is no longer configured for, and reports a write that in fact
    // succeeded as failed. The rollback must sit in the write's own catch.
    const handler = source.slice(
      source.indexOf('async function handleProviderChange'),
      source.indexOf('async function handleScheduleChange'),
    );
    const rollbacks = handler.match(/setSelectedProvider\(previous\)/g) ?? [];
    expect(rollbacks).toHaveLength(1);
    // The refresh's own failure says the save survived, so the user is not told
    // to redo work that is already done.
    expect(handler).toMatch(/showToast\(\s*`Backup provider saved, but/);
    const refreshIdx = handler.indexOf('getBackupProviders()');
    expect(refreshIdx).toBeGreaterThan(handler.indexOf('setSelectedProvider(previous)'));
  });

  it('refuses to write a provider while the schedule is unknown', () => {
    // One endpoint writes both keys, so a provider pick sends the schedule
    // too. A failed schedule load leaves it at the 'off' default, and writing
    // that would silently disable a real nightly backup.
    const handler = source.slice(
      source.indexOf('async function handleProviderChange'),
      source.indexOf('async function handleScheduleChange'),
    );
    expect(handler).toContain('if (!scheduleLoaded) return;');
    expect(source).toContain('disabled={!loadedProviders || !scheduleLoaded || backupPairSaving}');
  });

  it('disables both controls while either half of the pair is being written', () => {
    // Each handler sends the other half from captured state, so overlapping
    // writes let the later response overwrite one choice with a stale
    // counterpart. Two independent saving flags is what allowed the overlap.
    expect(source).toContain('const backupPairSaving = providerSaving || scheduleSaving;');
    expect(source).toContain('disabled={backupPairSaving || !selectedProvider}');
    expect(source).not.toContain('disabled={scheduleSaving || !selectedProvider}');
  });
});
