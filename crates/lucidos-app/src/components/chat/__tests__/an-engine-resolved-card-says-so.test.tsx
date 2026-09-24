// @vitest-environment jsdom
/** A permission card the engine resolved says so, rather than posing as a click.
 *
 *  The engine resolves a card itself in several cases: an unattended deny, a
 *  session that ended, a message that superseded it, a restart. Each one marks
 *  Deny as the surviving choice. Without the reason on screen, the user reads
 *  a Deny they never pressed. A real Deny click carries the user-click reason
 *  and draws no note, since the picked button already says it.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import {
  CommandPermissionBody,
  PermissionBody,
  engineResolutionNote,
} from '../PermissionCard';

const UNATTENDED =
  "Auto-denied: this coding-agent session runs unattended, and the command guard's static pass " +
  'refused to settle this request as safe. It refuses a command whose head is not what runs.';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

function note(): string | null {
  return host.querySelector('.permission-resolution-note')?.textContent ?? null;
}

describe('engineResolutionNote', () => {
  it('names the first sentence of an engine reason', () => {
    expect(engineResolutionNote({ allowed: false, reason: UNATTENDED })).toBe(
      "Auto-denied: this coding-agent session runs unattended, and the command guard's static pass " +
        'refused to settle this request as safe.',
    );
  });

  it('keeps a reason that is one sentence whole', () => {
    expect(engineResolutionNote({ allowed: false, reason: 'Superseded by a new message' })).toBe(
      'Superseded by a new message',
    );
  });

  it('draws nothing for a user click, or for no reason at all', () => {
    expect(engineResolutionNote({ allowed: false, reason: 'User denied' })).toBeNull();
    expect(engineResolutionNote({ allowed: true })).toBeNull();
    expect(engineResolutionNote(null)).toBeNull();
  });
});

describe('the coding-agent card', () => {
  const event = {
    request_id: 'r-1',
    tool_use_id: 't-1',
    tool_name: 'Bash',
    input: { command: "sed -i '' 's#a#b#' notes.md" },
    summary: "Bash sed -i '' 's#a#b#' notes.md",
  };

  it('shows why the engine denied it', () => {
    render(<PermissionBody event={event} resolved={{ allowed: false, reason: UNATTENDED }} />, host);
    expect(note()).toContain('runs unattended');
  });

  it('shows no note after the user pressed Deny', () => {
    render(<PermissionBody event={event} resolved={{ allowed: false, reason: 'User denied' }} />, host);
    expect(note()).toBeNull();
  });
});

describe('the command-guard card', () => {
  it('shows why the engine resolved it', () => {
    render(
      <CommandPermissionBody
        event={{ request_id: 'r-2', tool_use_id: 't-2', tool_name: 'run_bash', command: 'ls', summary: 'ls' }}
        resolved={{ allowed: false, reason: 'Superseded by a new message' }}
      />,
      host,
    );
    expect(note()).toBe('Superseded by a new message');
  });
});
