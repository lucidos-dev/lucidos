// @vitest-environment jsdom
/** A lasting choice shows only where the engine records one.
 *
 *  Both cards render exactly the offer the engine stamped on the event
 *  (ADR 0571), folded into the Remember menu. A card from before the stamp
 *  offers no lasting choice, so it draws no menu.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const resolveCommand = vi.fn((..._args: unknown[]) => new Promise<void>(() => {}));
vi.mock('../../../store/actions/permissions', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/permissions')>()),
  resolveCommandPermission: (...args: unknown[]) => resolveCommand(...args),
}));

import { CommandPermissionBody, PermissionBody, pendingVerdicts, resolvedChoice, denyText } from '../PermissionCard';
import type { GrantOffer } from '../../../generated/thread-event-wire';

const SECRET_NOTE = "This reads Lucidos's private files. No grant can cover that, so it can only be allowed once.";

function note(): string | null {
  return host.querySelector('.permission-resolution-note')?.textContent ?? null;
}

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  for (const id of ['r-1', 'r-2', 'r-3']) pendingVerdicts.clear(id);
  resolveCommand.mockClear();
});

/** Open the Remember menu and return its rows. The menu is portaled, so the
 *  rows sit under `document.body`. */
function openRemember(): HTMLElement[] {
  const face = host.querySelector<HTMLElement>('button.permission-remember');
  if (!face) return [];
  act(() => { face.click(); });
  return [...document.body.querySelectorAll<HTMLElement>('[role="menuitem"]')];
}

/** The lasting choices the card offers, in menu order. */
function remembered(): string[] {
  return openRemember().map((row) => row.textContent ?? '');
}

function labels(): string[] {
  return [...host.querySelectorAll('button')].map((b) => b.textContent ?? '');
}

/** The answer buttons alone, without the card's link to the guard setting. */
function answers(): string[] {
  return [...host.querySelectorAll('button.action-btn')].map((b) => b.textContent ?? '');
}

describe('the command-guard card', () => {
  const card = (command: string) => (
    <CommandPermissionBody
      event={{ request_id: 'r-1', tool_use_id: 't-1', tool_name: 'run_bash', command, summary: command }}
    />
  );

  it('offers no thread grant for a command with no readable head', () => {
    render(card('> /tmp/out'), host);
    expect(labels()).not.toContain('Allow for this thread');
    expect(labels()).toContain('Allow once');
  });

  it.each(['run_bash', 'run_bash_background', 'run_python'])('offers no unstamped scopes for %s', (toolName) => {
    render(<CommandPermissionBody event={{
      request_id: 'old-card', tool_use_id: 'old-tool', tool_name: toolName,
      command: 'git status', summary: 'Read status', grant_head: 'git',
    }} />, host);
    expect(answers()).toEqual(['Deny', 'Allow once']);
    expect(note()).toBe('This card comes from an older Lucidos, so it can only allow once.');
  });

  const stamped = (grantOffer: GrantOffer) => (
    <CommandPermissionBody
      event={{
        request_id: 'r-3',
        tool_use_id: 't-3',
        tool_name: 'run_bash',
        command: 'git status',
        summary: 'git status',
        grant_offer: grantOffer,
      }}
    />
  );

  it('renders the stamped offer, not its own derivation', () => {
    render(stamped({ session: 'Bash(git:*)', broad: 'Bash' }), host);
    expect(answers()).toEqual(['Remember', 'Deny', 'Allow once']);
    expect(remembered()).toEqual(['Allow for this thread', 'Always allow any shell command']);
  });

  it('offers Always deny last, and sends it as a denial that sets a rule', () => {
    render(stamped({ narrow: 'Bash(git:*)', deny: 'Bash(git:*)' }), host);
    const rows = openRemember();
    expect(rows.map((row) => row.textContent)).toEqual(['Always allow git commands', 'Always deny git commands']);
    act(() => { rows[1].click(); });
    expect(resolveCommand).toHaveBeenCalledWith('r-3', false, undefined, true);
    expect(answers()).toEqual(['Always deny git commands', 'Deny', 'Allow once']);
    expect(host.querySelector('button.permission-remember')).toBeNull();
  });

  it('offers Always deny on a secret read, where no allow is offered', () => {
    render(stamped({ withheld: 'secret-read', deny: 'Bash(cat:*)' }), host);
    expect(remembered()).toEqual(['Always deny cat commands']);
    expect(note()).toBe(SECRET_NOTE);
  });

  it('shows a spent Remember button once Deny or Allow once is picked', () => {
    render(
      <CommandPermissionBody
        event={{ request_id: 'r-3', tool_use_id: 't-3', tool_name: 'run_bash', command: 'git status', summary: 'git status', grant_offer: { deny: 'Bash(git:*)' } }}
        resolved={{ allowed: true }}
      />,
      host,
    );
    expect(answers()).toEqual(['Remember', 'Deny', '✓ Allow once']);
    expect(host.querySelector('button.permission-remember')).toBeNull();
    expect([...host.querySelectorAll<HTMLButtonElement>('button.action-btn')].every((b) => b.disabled)).toBe(true);
  });

  it('marks Always deny as the pick of a resolution that stored a rule', () => {
    expect(resolvedChoice({ allowed: false, reason: 'User denied', standing_deny: 'Bash(git:*)' })).toBe('always-deny');
    expect(resolvedChoice({ allowed: false, reason: 'User denied' })).toBe('deny');
    expect(denyText('Python')).toBe('Always deny Python');
    expect(denyText('Bash')).toBe('Always deny any shell command');
  });

  it('keeps an empty engine stamp without an older-card note', () => {
    render(stamped({}), host);
    expect(answers()).toEqual(['Deny', 'Allow once']);
    expect(note()).toBeNull();
  });

  it('offers only Allow once for a secret read, and says why', () => {
    render(stamped({ withheld: 'secret-read' }), host);
    expect(answers()).toEqual(['Deny', 'Allow once']);
    expect(note()).toBe(SECRET_NOTE);
  });
});

describe('the coding-agent card', () => {
  const card = (grantOffer?: GrantOffer) => (
    <PermissionBody
      event={{
        request_id: 'r-2',
        tool_use_id: 't-2',
        tool_name: 'command_execution',
        input: { command: "/bin/zsh -lc './scripts/check.sh'", cwd: '/ws' },
        summary: 'command_execution ./scripts/check.sh',
        grant_offer: grantOffer,
      }}
    />
  );

  it('renders exactly the grants the engine stamped', () => {
    render(card({
      broad: 'Bash',
      narrow: 'Bash(./scripts/check.sh:*)',
      session: 'command_execution(./scripts/check.sh:*)',
    }), host);
    expect(answers()).toEqual(['Remember', 'Deny', 'Allow once']);
    expect(remembered()).toEqual([
      'Allow for this thread',
      'Always allow ./scripts/check.sh commands',
      'Always allow Bash',
    ]);
  });

  it('offers only what was stamped, so a missing pattern is no button', () => {
    render(card({ broad: 'Bash' }), host);
    expect(answers()).toEqual(['Remember', 'Deny', 'Allow once']);
    expect(remembered()).toEqual(['Always allow Bash']);
  });

  it('offers no grant on a card from before the engine stamped one', () => {
    render(card(undefined), host);
    expect(answers()).toEqual(['Deny', 'Allow once']);
    expect(note()).toBeNull();
  });

  it('labels the fallback and offers only Deny and Allow once', () => {
    render(card({ withheld: 'standing-deny-unavailable' }), host);
    expect(answers()).toEqual(['Deny', 'Allow once']);
    expect(note()).toBe('Couldn’t check Always deny rules. Review this command before allowing it once.');
  });

  it('offers only Allow once for a secret read, and says why', () => {
    render(card({ withheld: 'secret-read' }), host);
    expect(answers()).toEqual(['Deny', 'Allow once']);
    expect(note()).toBe(SECRET_NOTE);
  });
});
