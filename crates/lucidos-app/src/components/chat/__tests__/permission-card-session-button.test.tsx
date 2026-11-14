// @vitest-environment jsdom
/** A grant button shows only where the engine records a grant.
 *
 *  Both cards render exactly the offer the engine stamped on the event
 *  (ADR 0571). A chat card from before the stamp derives its own: a command
 *  with no readable head, such as a bare redirect, gets no session button.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import { CommandPermissionBody, PermissionBody } from '../PermissionCard';
import type { GrantOffer } from '../../../generated/thread-event-wire';

const SECRET_NOTE = 'This reads a secret, so Lucidos can only allow it once.';

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
});

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

  it('offers it when a head derives', () => {
    render(card('git status'), host);
    expect(labels()).toContain('Allow for this thread');
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
    expect(answers()).toEqual(['Deny', 'Allow once', 'Allow for this thread', 'Always allow']);
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
    expect(answers()).toEqual([
      'Deny',
      'Allow once',
      'Allow for this thread',
      'Always allow ./scripts/check.sh commands',
      'Always allow',
    ]);
  });

  it('offers only what was stamped, so a missing pattern is no button', () => {
    render(card({ broad: 'Bash' }), host);
    expect(answers()).toEqual(['Deny', 'Allow once', 'Always allow']);
  });

  it('offers no grant on a card from before the engine stamped one', () => {
    render(card(undefined), host);
    expect(answers()).toEqual(['Deny', 'Allow once']);
    expect(note()).toBeNull();
  });

  it('offers only Allow once for a secret read, and says why', () => {
    render(card({ withheld: 'secret-read' }), host);
    expect(answers()).toEqual(['Deny', 'Allow once']);
    expect(note()).toBe(SECRET_NOTE);
  });
});
