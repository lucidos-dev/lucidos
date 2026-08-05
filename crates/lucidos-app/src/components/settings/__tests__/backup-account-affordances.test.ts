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
