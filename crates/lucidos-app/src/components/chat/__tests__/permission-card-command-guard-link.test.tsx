// @vitest-environment jsdom
/** A pending command card links to the Command guard's Enabled for coding agents setting.
 *
 *  Only a command tool carries it, only while the card waits for an answer,
 *  and only while the switch is off: once on, a card means the guard asked. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';

const openSettings = vi.hoisted(() => vi.fn());
vi.mock('../../../store/actions/menu', () => ({
  openCommandGuardCodingAgentsSettings: openSettings,
}));
const codingAgentsOn = vi.hoisted(() => vi.fn(() => false));
vi.mock('../../../store/actions/preferences', () => ({
  currentCommandGuardCodingAgents: codingAgentsOn,
}));

import { PermissionBody } from '../PermissionCard';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  openSettings.mockClear();
  codingAgentsOn.mockReturnValue(false);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

const LINK = '.permission-settings-link';

function card(tool_name: string, resolved?: { allowed: boolean }) {
  return (
    <PermissionBody
      event={{ request_id: 'r-1', tool_use_id: 't-1', tool_name, input: {}, summary: 'x' }}
      resolved={resolved}
    />
  );
}

describe('the command guard link on a coding-agent permission card', () => {
  it.each(['Bash', 'command_execution'])('shows on a pending %s card and opens the setting', (tool) => {
    render(card(tool), host);
    const link = host.querySelector<HTMLButtonElement>(LINK);
    expect(link).not.toBeNull();
    link!.click();
    expect(openSettings).toHaveBeenCalledOnce();
  });

  it.each(['file_change', 'Edit', 'Skill'])('is absent from a %s card', (tool) => {
    render(card(tool), host);
    expect(host.querySelector(LINK)).toBeNull();
  });

  it('names what it opens', () => {
    render(card('Bash'), host);
    expect(host.querySelector(LINK)?.textContent).toBe('Let the guard answer these');
  });

  it('is absent once the switch is already on', () => {
    codingAgentsOn.mockReturnValue(true);
    render(card('Bash'), host);
    expect(host.querySelector(LINK)).toBeNull();
  });

  it('goes away once the card is answered', () => {
    render(card('Bash', { allowed: true }), host);
    expect(host.querySelector(LINK)).toBeNull();
  });

  it('is no arrow-key choice', () => {
    render(card('Bash'), host);
    expect(host.querySelector(LINK)?.hasAttribute('data-not-choice')).toBe(true);
  });
});
