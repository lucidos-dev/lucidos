// @vitest-environment jsdom
/** A pending command card links to the Auto-answer coding-agent cards switch.
 *
 *  Only a command tool carries it, only while the card waits for an answer,
 *  and only while auto-answer is off. The command guard switch plays no part.
 *  With auto-answer on, the card links to the judge model instead, but only
 *  while no judge model resolves. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';

const openSettings = vi.hoisted(() => vi.fn());
const openJudgeModel = vi.hoisted(() => vi.fn());
vi.mock('../../../store/actions/menu', () => ({
  openCodingAgentAutoAnswerSettings: openSettings,
  openCommandJudgeModelSettings: openJudgeModel,
}));
const judgeUnresolved = vi.hoisted(() => ({ why: null as string | null }));
vi.mock('../../settings/judgmentBackend', () => ({
  JUDGMENT_MODEL_KEY: { 'command-guard': 'model_command_judge' },
}));
vi.mock('../../settings/useBackgroundModels', () => ({
  useBackgroundModels: () => ({
    row: () => ({ unresolved: judgeUnresolved.why }),
    error: null,
    refresh: () => Promise.resolve(),
  }),
}));
const autoAnswerOn = vi.hoisted(() => vi.fn(() => false));
vi.mock('../../../store/actions/preferences', () => ({
  currentCodingAgentAutoAnswer: autoAnswerOn,
}));

import { PermissionBody } from '../PermissionCard';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  openSettings.mockClear();
  openJudgeModel.mockClear();
  autoAnswerOn.mockReturnValue(false);
  judgeUnresolved.why = null;
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

describe('the auto-answer link on a coding-agent permission card', () => {
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
    expect(host.querySelector(LINK)?.textContent).toBe('Let Lucidos answer these');
  });

  it('is absent once auto-answer is on', () => {
    autoAnswerOn.mockReturnValue(true);
    render(card('Bash'), host);
    expect(host.querySelector(LINK)).toBeNull();
  });

  it('points at the judge model while auto-answer is on and no judge model resolves', () => {
    autoAnswerOn.mockReturnValue(true);
    judgeUnresolved.why = 'No configured provider serves a command judge model.';
    render(card('Bash'), host);
    const link = host.querySelector<HTMLButtonElement>(LINK);
    expect(link?.textContent).toBe('Pick a judge model so Lucidos can answer');
    expect(link?.dataset.tooltip).toBe(judgeUnresolved.why);
    link!.click();
    expect(openJudgeModel).toHaveBeenCalledOnce();
    expect(openSettings).not.toHaveBeenCalled();
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
