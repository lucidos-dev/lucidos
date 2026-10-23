// @vitest-environment jsdom
/** "Allow for this thread" shows only where the engine records a grant.
 *
 *  A command with no readable head, such as a bare redirect, derives no session
 *  pattern on either lane (`derive_command_allow_pattern`, `derive_allow_pattern`).
 *  A button there would run the command once while claiming a thread grant.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import { CommandPermissionBody, PermissionBody, sessionGrantable } from '../PermissionCard';

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

describe('sessionGrantable', () => {
  it('needs a head for a command and a path for a path tool', () => {
    expect(sessionGrantable('Bash', { command: 'git status' })).toBe(true);
    expect(sessionGrantable('Bash', { command: '> /tmp/out' })).toBe(false);
    expect(sessionGrantable('command_execution', { command: '> /tmp/out' })).toBe(false);
    expect(sessionGrantable('Edit', { file_path: '/tmp/a.md' })).toBe(true);
    expect(sessionGrantable('Edit', {})).toBe(false);
  });

  it('grants a bare tool name to a tool with no key, and never file_change', () => {
    expect(sessionGrantable('Read', {})).toBe(true);
    expect(sessionGrantable('file_change', {})).toBe(false);
  });
});

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
});

describe('the coding-agent card', () => {
  it('offers no thread grant for a Bash command with no readable head', () => {
    render(
      <PermissionBody
        event={{
          request_id: 'r-2',
          tool_use_id: 't-2',
          tool_name: 'Bash',
          input: { command: '> /tmp/out' },
          summary: 'Bash > /tmp/out',
        }}
      />,
      host,
    );
    expect(labels()).not.toContain('Allow for this thread');
  });
});
