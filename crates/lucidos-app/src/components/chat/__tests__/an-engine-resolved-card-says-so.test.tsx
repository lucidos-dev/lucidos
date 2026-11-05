// @vitest-environment jsdom
/** A permission card the engine resolved says so, rather than posing as a click.
 *
 *  The engine resolves a card itself in several cases: an unattended deny, a
 *  session that ended, a message that superseded it, a restart. None of those
 *  is a Deny the user pressed, so no button reads as picked, and a note in
 *  plain words says what happened. A real Deny click carries the user-click
 *  reason and draws no note, since the picked button already says it.
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
  it('says a known engine reason in plain words', () => {
    expect(engineResolutionNote({ allowed: false, reason: UNATTENDED })).toBe(
      'Refused by Lucidos: its command check could not confirm this was safe, and nobody was there to ask.',
    );
    expect(engineResolutionNote({ allowed: false, reason: 'Superseded by a new message' })).toBe(
      'Closed because you sent a new message instead.',
    );
  });

  // A reason the card has no words for yet is never silent.
  it('falls back to the first sentence of a reason it cannot translate', () => {
    expect(engineResolutionNote({ allowed: false, reason: 'A new reason. With more after it.' }))
      .toBe('A new reason.');
    expect(engineResolutionNote({ allowed: false, reason: 'One sentence' })).toBe('One sentence');
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

  it('shows why the engine denied it, and marks no button as pressed', () => {
    render(<PermissionBody event={event} resolved={{ allowed: false, reason: UNATTENDED }} />, host);
    expect(note()).toContain('nobody was there to ask');
    expect(host.querySelector('.permission-btn-picked')).toBeNull();
  });

  it('still marks Deny when the user pressed it', () => {
    render(<PermissionBody event={event} resolved={{ allowed: false, reason: 'User denied' }} />, host);
    expect(host.querySelector('.permission-btn-picked')?.textContent).toContain('Deny');
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
    expect(note()).toBe('Closed because you sent a new message instead.');
  });
});
