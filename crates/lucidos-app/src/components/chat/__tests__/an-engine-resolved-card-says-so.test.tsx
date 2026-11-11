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
  LEGACY_RESOLUTION_NOTES,
  PLAIN_RESOLUTION_NOTES,
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

  // The first matching opening wins, so a prefix would shadow a longer one.
  it('has no opening that is a prefix of another', () => {
    const notes = [...PLAIN_RESOLUTION_NOTES, ...LEGACY_RESOLUTION_NOTES];
    for (const [a] of notes) {
      for (const [b] of notes) {
        if (a !== b) expect(b.startsWith(a), `"${a}" shadows "${b}"`).toBe(false);
      }
    }
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

  // A parked card that ran out its wait was shown to someone, so it never
  // claims nobody was there to ask.
  it('says nobody answered a parked card in time', () => {
    const expired = 'Auto-denied: nobody answered this permission card within its permission wait'; // a legacy stored reason
    render(<PermissionBody event={event} resolved={{ allowed: false, reason: expired }} />, host);
    expect(note()).toBe('Refused by Lucidos: nobody answered this card in time.');
    expect(note()).not.toContain('nobody was there to ask');
    expect(host.querySelector('.permission-btn-picked')).toBeNull();
  });

  // An ungranted side effect was refused by policy, not for want of someone
  // to ask, and an engine-started run has no trigger to blame.
  it('says an unattended run had no permission for a side effect', () => {
    const ungranted =
      'Auto-denied: this unattended coding-agent session has no side-effect grant for sending ' +
      "email. A trigger's side-effect grant can allow it. A run the engine started has none.";
    render(<PermissionBody event={event} resolved={{ allowed: false, reason: ungranted }} />, host);
    expect(note()).toBe('Refused by Lucidos: this unattended run has no permission for this kind of action.');
    expect(note()).not.toContain('nobody was there to ask');
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
