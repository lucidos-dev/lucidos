// @vitest-environment jsdom
/**
 * A pick on a started thread asks before it rebuilds the thread's cache. Keep
 * and a dismissal send nothing. "Switch anyway" is the only path that records
 * the pick on the engine, and the handoff starts a successor on the picked rung.
 */
import { beforeAll, describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import type { CodingAgentCommandsResponse, SwitchEstimate } from '../../../api/client';

const response: CodingAgentCommandsResponse = {
  control_commands: [{
    subtype: 'set_model',
    label: 'Model',
    params: [{ key: 'model', options: [
      { value: 'default', label: 'Default', description: '' },
      { value: 'claude-opus-5-6', label: 'Opus 5.6', description: '', reasoning_efforts: ['medium', 'high'] },
    ] }],
  }, {
    subtype: 'set_reasoning_effort',
    label: 'Reasoning effort',
    params: [{ key: 'effort', options: [
      { value: 'medium', label: 'Medium', description: 'Balanced' },
      { value: 'high', label: 'High', description: 'Deeper' },
    ] }],
  }],
  builtin_commands: [],
  skill_commands: [],
  current_model: 'claude-opus-5-6',
  current_reasoning_effort: 'medium',
  has_active_session: true,
  default_implementer: { model: 'claude-opus-5-6', effort: 'medium' },
  ladder_rungs: [
    { model: 'claude-opus-5-6', effort: 'medium', label: 'Opus 5.6', estimated: true },
    { model: 'claude-opus-5-6', effort: 'high', label: 'Opus 5.6', estimated: true },
  ],
} as unknown as CodingAgentCommandsResponse;

let hasActiveSession = true;
const sendCodingAgentControl = vi.fn(() => Promise.resolve(true));
const fetchSwitchEstimate = vi.fn<() => Promise<SwitchEstimate>>();
const continueInSuccessor = vi.fn((_threadId: string, _message: string | undefined, _implementer?: unknown) =>
  Promise.resolve({ path: 'brief' as const, successor_thread_id: 's1', link: '[Next](thread:ws/s1)' }));
const updateCompose = vi.fn();
const focusThreadOrBootstrap = vi.fn();

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  fetchCodingAgentCommands: vi.fn(() => Promise.resolve({ ...response, has_active_session: hasActiveSession })),
  fetchSwitchEstimate,
  continueInSuccessor,
}));
vi.mock('../../../store/actions/chat-claude-code', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/chat-claude-code')>()),
  sendCodingAgentControl,
}));
vi.mock('../../../store/actions/compose', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/compose')>()),
  updateCompose,
}));
vi.mock('../../../store/actions/threads', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/threads')>()),
  focusThreadOrBootstrap,
}));

const { RungControl } = await import('../RungControl');
const { passSendGate, sendGateAsks } = await import('../rungSwitch');
const { ConfirmDialog } = await import('../../shared/ConfirmDialog');
const { controlMenuParts } = await import('../controlMenuParts');
const { confirmState, settleConfirm, successorCardPick, successorCardThreadId, threadMap, toasts } =
  await import('../../../store/store');
const { patchDraft, clearDraft } = await import('../../../store/composeDrafts');
const { makeThreadState } = await import('../../../store/actions/threads-test-helpers');

beforeAll(() => controlMenuParts.preload());

const REBUILD: SwitchEstimate = { context_tokens: 300_000, rebuilds_cache: true, extra_cost_usd: 0.42 };
const KEEPS_CACHE: SwitchEstimate = { context_tokens: 300_000, rebuilds_cache: false, extra_cost_usd: null };

let host: HTMLDivElement;

async function settle(): Promise<void> {
  for (let i = 0; i < 40; i++) await new Promise(r => setTimeout(r, 0));
}

function click(selector: string, text?: string): void {
  const el = Array.from(document.querySelectorAll<HTMLElement>(selector))
    .find(e => text === undefined || e.textContent?.includes(text));
  if (!el) throw new Error(`no ${selector}${text ? ` with "${text}"` : ''}`);
  el.click();
}

function seedThread() {
  threadMap.value = new Map([['t1', makeThreadState('t1', { meta: { channel: 'claude_code', status: 'idle' } })]]);
}

/** Pick Opus 5.6 · high from the pill's list, then wait for what follows. */
async function pickHigh(): Promise<void> {
  click('.rung-trigger');
  await settle();
  click('.control-option', 'Opus 5.6 · high');
  await settle();
}

beforeEach(() => {
  vi.clearAllMocks();
  hasActiveSession = true;
  seedThread();
  host = document.createElement('div');
  document.body.appendChild(host);
  render(<><RungControl threadId="t1" codingAgent="claude-code" /><ConfirmDialog /></>, host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  settleConfirm(false);
  clearDraft('t1');
  threadMap.value = new Map();
  successorCardThreadId.value = null;
  successorCardPick.value = null;
  toasts.value = [];
});

async function startedPick(estimate: SwitchEstimate): Promise<void> {
  fetchSwitchEstimate.mockResolvedValue(estimate);
  await settle();
  await pickHigh();
}

describe('a rung pick on a started idle thread', () => {
  it('asks the engine what the pick costs, and switches silently when the cache is kept', async () => {
    await startedPick(KEEPS_CACHE);
    expect(fetchSwitchEstimate).toHaveBeenCalledWith('t1', 'claude-opus-5-6', 'high');
    expect(confirmState.value.visible).toBe(false);
    expect(sendCodingAgentControl).toHaveBeenCalledWith('t1', { subtype: 'set_model', model: 'claude-opus-5-6', effort: 'high' });
  });

  it('asks first when the pick rebuilds the cache, and sends nothing before an answer', async () => {
    await startedPick(REBUILD);
    expect(confirmState.value.visible).toBe(true);
    const text = document.querySelector('.confirm-dialog')!.textContent!;
    expect(text).toContain('one time');
    expect(text).toContain('about $0.42');
    expect(text).not.toMatch(/slow/i);
    expect(sendCodingAgentControl).not.toHaveBeenCalled();
    expect(continueInSuccessor).not.toHaveBeenCalled();
  });

  it('offers the handoff and the switch as two choice cards, the handoff first, and Keep in the foot', async () => {
    await startedPick(REBUILD);
    const cards = Array.from(document.querySelectorAll('.confirm-choice'));
    expect(cards.map(c => c.getAttribute('data-role'))).toEqual(['confirm-ok', 'confirm-extra']);
    expect(cards[0].textContent).toBe('Hand off to a new threadStarts fresh on Opus 5.6 · high and carries on from where this one stands.');
    expect(cards[1].textContent).toContain('Switch this thread');
    expect(cards[1].textContent).toContain('about $0.42');
    expect(document.activeElement).toBe(cards[0]);
    expect(document.querySelector('.surface-foot')!.textContent).toBe('Keep Opus 5.6 · medium');
  });

  it('sends nothing on Keep', async () => {
    await startedPick(REBUILD);
    click('[data-role="confirm-cancel"]');
    await settle();
    expect(sendCodingAgentControl).not.toHaveBeenCalled();
    expect(continueInSuccessor).not.toHaveBeenCalled();
  });

  it('sends nothing when the dialog is dismissed', async () => {
    await startedPick(REBUILD);
    // Escape and an outside click both settle the confirm as `false`.
    settleConfirm(false);
    await settle();
    expect(sendCodingAgentControl).not.toHaveBeenCalled();
    expect(continueInSuccessor).not.toHaveBeenCalled();
  });

  it('records the pick, model and effort in one request, only on Switch anyway', async () => {
    await startedPick(REBUILD);
    click('[data-role="confirm-extra"]');
    await settle();
    expect(sendCodingAgentControl).toHaveBeenCalledTimes(1);
    expect(sendCodingAgentControl).toHaveBeenCalledWith('t1', { subtype: 'set_model', model: 'claude-opus-5-6', effort: 'high' });
    expect(continueInSuccessor).not.toHaveBeenCalled();
  });

  it('hands off to a new thread on the picked rung, with no message, and switches nothing', async () => {
    await startedPick(REBUILD);
    click('[data-role="confirm-ok"]');
    await settle();
    expect(continueInSuccessor).toHaveBeenCalledWith('t1', undefined, {
      coding_agent_model: 'claude-opus-5-6',
      reasoning_effort: 'high',
    });
    expect(focusThreadOrBootstrap).toHaveBeenCalledWith('s1');
    expect(sendCodingAgentControl).not.toHaveBeenCalled();
    expect(updateCompose).not.toHaveBeenCalled();
  });

  it('hands the message in the composer to the successor, and takes it', async () => {
    patchDraft('t1', { text: '  Add a dark-mode toggle ' });
    await startedPick(REBUILD);
    click('[data-role="confirm-ok"]');
    await settle();
    expect(continueInSuccessor).toHaveBeenCalledWith('t1', 'Add a dark-mode toggle', expect.anything());
    expect(updateCompose).toHaveBeenCalledWith('t1', { text: '' });
  });

  it('only aims the successor while the Successor card shows: no estimate, no dialog, no switch', async () => {
    successorCardThreadId.value = 't1';
    await startedPick(REBUILD);
    expect(fetchSwitchEstimate).not.toHaveBeenCalled();
    expect(confirmState.value.visible).toBe(false);
    expect(sendCodingAgentControl).not.toHaveBeenCalled();
    expect(successorCardPick.value).toEqual({ threadId: 't1', model: 'claude-opus-5-6', effort: 'high' });
    expect(document.querySelector('.rung-trigger')!.textContent).toContain('high');
  });

  describe('a send in the thread while the Successor card holds a pick', () => {
    async function underCardPick(): Promise<void> {
      successorCardThreadId.value = 't1';
      await startedPick(REBUILD);
      fetchSwitchEstimate.mockClear();
    }

    it('asks the rung switch question the pick skipped, and cancels the send on Keep', async () => {
      await underCardPick();
      const sent = passSendGate('t1');
      await settle();
      expect(fetchSwitchEstimate).toHaveBeenCalledWith('t1', 'claude-opus-5-6', 'high');
      expect(confirmState.value.visible).toBe(true);
      click('[data-role="confirm-cancel"]');
      expect(await sent).toBe(false);
      expect(sendCodingAgentControl).not.toHaveBeenCalled();
    });

    it('records the pick here, then lets the send go, on Switch here', async () => {
      await underCardPick();
      const sent = passSendGate('t1');
      await settle();
      click('[data-role="confirm-extra"]');
      expect(await sent).toBe(true);
      expect(sendCodingAgentControl).toHaveBeenCalledWith('t1', { subtype: 'set_model', model: 'claude-opus-5-6', effort: 'high' });
      expect(successorCardPick.value).toBeNull();
    });

    it('asks even when the cache is kept, since the pick was meant for the successor', async () => {
      await underCardPick();
      fetchSwitchEstimate.mockResolvedValue(KEEPS_CACHE);
      const sent = passSendGate('t1');
      await settle();
      expect(confirmState.value.visible).toBe(true);
      click('[data-role="confirm-cancel"]');
      expect(await sent).toBe(false);
      expect(sendCodingAgentControl).not.toHaveBeenCalled();
    });

    it('cancels the send when recording the pick fails', async () => {
      await underCardPick();
      sendCodingAgentControl.mockResolvedValueOnce(false);
      const sent = passSendGate('t1');
      await settle();
      click('[data-role="confirm-extra"]');
      expect(await sent).toBe(false);
    });

    it('hands off in place of the send, and cancels it', async () => {
      await underCardPick();
      const sent = passSendGate('t1');
      await settle();
      click('[data-role="confirm-ok"]');
      expect(await sent).toBe(false);
      expect(continueInSuccessor).toHaveBeenCalled();
    });

    it('asks nothing when no pick is held', async () => {
      successorCardThreadId.value = 't1';
      await settle();
      expect(sendGateAsks('t1')).toBe(false);
      expect(await passSendGate('t1')).toBe(true);
      expect(fetchSwitchEstimate).not.toHaveBeenCalled();
    });

    it('asks nothing once the card is gone', async () => {
      await underCardPick();
      expect(sendGateAsks('t1')).toBe(true);
      successorCardThreadId.value = null;
      expect(sendGateAsks('t1')).toBe(false);
      expect(await passSendGate('t1')).toBe(true);
      expect(fetchSwitchEstimate).not.toHaveBeenCalled();
    });
  });

  it('says so, and switches nothing, when the engine cannot price the switch', async () => {
    fetchSwitchEstimate.mockRejectedValue(new Error('engine down'));
    await settle();
    await pickHigh();
    expect(confirmState.value.visible).toBe(false);
    expect(sendCodingAgentControl).not.toHaveBeenCalled();
    expect(toasts.value.map(t => t.message).join(' ')).toContain('Could not check what switching to Opus 5.6 · high costs');
  });
});

describe('a rung pick on an idle thread whose session has exited', () => {
  beforeEach(async () => {
    render(null, host);
    hasActiveSession = false;
    render(<><RungControl threadId="t1" codingAgent="claude-code" /><ConfirmDialog /></>, host);
    await settle();
  });

  it('records the pick on the engine on Switch anyway, as on a live thread', async () => {
    await startedPick(REBUILD);
    click('[data-role="confirm-extra"]');
    await settle();
    expect(sendCodingAgentControl).toHaveBeenCalledTimes(1);
    expect(sendCodingAgentControl).toHaveBeenCalledWith('t1', { subtype: 'set_model', model: 'claude-opus-5-6', effort: 'high' });
  });

  it('records the pick silently when the cache is cold, with no confirm', async () => {
    await startedPick(KEEPS_CACHE);
    expect(confirmState.value.visible).toBe(false);
    expect(sendCodingAgentControl).toHaveBeenCalledTimes(1);
  });
});
