import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));

function code(path: string): string {
  return readFileSync(resolve(here, path), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
}

const source = code('../BackupSection.tsx');
const seedingSource = code('../backupSeeding.ts');
const menuSource = code('../../../store/actions/menu.ts');

describe('the not-connected state offers a way out, not a path to walk', () => {
  it('wires a button to the Connected accounts deep-link', () => {
    expect(source).toContain('openConnectedAccountsSettings');
    expect(source).toMatch(/onClick=\{[\s\S]*?openConnectedAccountsSettings\(/);
  });

  it('carries the provider and the scopes an upload needs', () => {
    const call = source.slice(
      source.indexOf('openConnectedAccountsSettings('),
      source.indexOf('Connect {providerInfo.name}'),
    );
    expect(call).toContain('oauthProviderFor(providerInfo.id)');
    expect(call).toContain('PROVIDER_SCOPES[providerInfo.id]');
  });

  it('no longer instructs the user to navigate there themselves', () => {
    expect(source).not.toMatch(/account in Settings . Accounts/);
  });

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

  it('names connecting the account and saving the key', () => {
    const prompt = source.slice(
      source.indexOf('askLucidosToSetUpBackups'),
      source.indexOf('askLucidosToSetUpBackups') + 900,
    );
    expect(prompt).toContain('connect the account');
    expect(prompt).toContain('backup key');
  });

  it('routes through handleNavigationRequest', () => {
    expect(source).toContain('handleNavigationRequest({');
  });
});

describe('the provider dropdown reflects and writes the configured destination', () => {
  it('seeds through the shared decision instead of the first registry entry', () => {
    expect(source).toContain('backupSeed(');
    expect(seedingSource).toContain('pickInitialProvider(');
    expect(source).not.toMatch(/setSelectedProvider\(\s*p\[0\]\.id\s*\)/);
  });

  it('settles both requests before selecting, so the fallback cannot win a race', () => {
    expect(source).toContain('Promise.allSettled([');
    expect(source).toContain('getBackupProviders()');
    expect(source).toContain('getBackupSchedule()');
  });

  it('persists a pick instead of holding it as view-only state', () => {
    expect(source).toMatch(/onChange=\{\(v\) => void handleProviderChange\(v\)\}/);
    expect(source).toContain('async function handleProviderChange');
    expect(source).not.toMatch(/onChange=\{\(v\) => setSelectedProvider\(v\)\}/);
  });

  it('sends the current schedule with the new provider', () => {
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
    const handler = source.slice(
      source.indexOf('async function handleProviderChange'),
      source.indexOf('async function handleScheduleChange'),
    );
    const rollbacks = handler.match(/setSelectedProvider\(previous\)/g) ?? [];
    expect(rollbacks).toHaveLength(1);
    expect(handler).toMatch(/showToast\(\s*`Backup provider saved, but/);
    const refreshIdx = handler.indexOf('getBackupProviders()');
    expect(refreshIdx).toBeGreaterThan(handler.indexOf('setSelectedProvider(previous)'));
  });

  it('refuses to write a provider while the schedule is unknown', () => {
    const handler = source.slice(
      source.indexOf('async function handleProviderChange'),
      source.indexOf('async function handleScheduleChange'),
    );
    expect(handler).toContain('if (!scheduleLoaded) return;');
    expect(source).toContain('disabled={!loadedProviders || !scheduleLoaded || backupPairSaving}');
  });

  it('disables both controls while either half of the pair is being written', () => {
    expect(source).toContain('const backupPairSaving = providerSaving || scheduleSaving;');
    expect(source).toContain('disabled={backupPairSaving || !selectedProvider}');
    expect(source).not.toContain('disabled={scheduleSaving || !selectedProvider}');
  });
});

describe('the Backup section is laid out by CSS, not by inline styles', () => {
  const raw = readFileSync(resolve(here, '../BackupSection.tsx'), 'utf8');
  const keyPanel = readFileSync(resolve(here, '../BackupKeyPanel.tsx'), 'utf8');

  it('carries no static inline style attribute', () => {
    const inline = raw.match(/style="[^"]*"/g) ?? [];
    expect(inline).toEqual([]);
  });

  it('gives every row a class', () => {
    for (const cls of [
      'backup-blocked-state',
      'backup-actions-row',
    ]) {
      expect(raw).toContain(`class="${cls}"`);
    }
    for (const cls of ['backup-key', 'backup-key-actions', 'backup-key-value', 'backup-key-warning']) {
      expect(keyPanel).toContain(`class="${cls}"`);
    }
  });

  it('renders both blocked states through the same class', () => {
    const blocked = raw.match(/class="backup-blocked-state"/g) ?? [];
    expect(blocked).toHaveLength(2);
    expect(raw).not.toContain('backup-not-connected-row');
  });

  it('names the missing permissions through the shared line', () => {
    expect(source).toContain('backupAccessLine(providerInfo.name, providerInfo.missing_scopes)');
    expect(source).not.toContain('access not granted');
  });

  it('puts the blocked-state button on its own line under the message', () => {
    const css = code('../../../styles/settings/backup.css');
    const rule = css.match(/\.backup-blocked-state\s*\{([^}]*)\}/)?.[1] ?? '';
    expect(rule).toContain('flex-direction: column');
    expect(rule).not.toContain('flex-wrap');
  });
});
