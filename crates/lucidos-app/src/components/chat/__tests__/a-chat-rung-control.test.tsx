// @vitest-environment jsdom
/**
 * The Lucidos Agent's rung control: the same split button and pill as a coding
 * agent's, listing the chat ladder's rungs. A draft's pick is the draft's own.
 * A pick on a started thread asks the engine whether it rebuilds a warm cache,
 * and asks the user before it does. A chat thread has no successor, so the
 * choice is Keep or Switch anyway. The engine records the pick, and refuses it
 * while a turn runs, when the pill refuses every pick and step.
 */
import { beforeAll, describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import type { ChatSettingsPick, SwitchEstimate } from '../../../api/client';
import type { IntelligenceLadder, ModelInfo, ServedIntelligenceRung } from '../../../api/types';

import { DEFAULT_CHAT_MODEL } from '../../../store/models';

/** The default chat model is Opus 5.5, whose label the pill reads from the
 *  fallback model list. */
const OPUS = DEFAULT_CHAT_MODEL;

const fetchSwitchEstimate = vi.fn<() => Promise<SwitchEstimate>>();
const recordChatSettings = vi.fn<(threadId: string, pick: ChatSettingsPick) => Promise<void>>(async () => {});

const served = (model: string, label: string, effort: string, on = true): ServedIntelligenceRung => ({
  model, label, effort, on, estimated: true, intelligence: null, cost: null, time: null, is_default: false,
});

/** The chat ladder: Haiku low and Opus medium and high on, Sonnet off. Read
 *  at call time, once the fallback model list has loaded. */
const servedChatLadder = (): IntelligenceLadder => ({
  surface: 'chat',
  source: 'test',
  estimated: '2026-10-16',
  customised: false,
  climbs: true,
  shipped_rungs: [],
  default_rung: { model: OPUS, effort: 'medium' },
  reference: null,
  long_prompt_share: null,
  rungs: [
    served(HAIKU, 'Haiku 5.5', 'low'),
    served(SONNET, 'Sonnet 5.5', 'medium', false),
    served(OPUS, 'Opus 5.5', 'medium'),
    served(OPUS, 'Opus 5.5', 'high'),
  ],
});

const getChatLadder = vi.fn(async () => servedChatLadder());

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  fetchSwitchEstimate,
  getChatLadder,
  recordChatSettings,
}));
const openIntelligenceLadder = vi.fn();
vi.mock('../../../store/actions/menu', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/menu')>()),
  openIntelligenceLadder,
}));
vi.mock('../../../store/actions/models', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/models')>()),
  loadChatModels: vi.fn(async () => {}),
  rememberProvider: vi.fn(async () => {}),
}));

const { RungControl } = await import('../RungControl');
const { ConfirmDialog } = await import('../../shared/ConfirmDialog');
const { controlMenuParts } = await import('../controlMenuParts');
const {
  confirmState, settleConfirm, threadMap, toasts, currentModel, chatModels, chatLadder,
} = await import('../../../store/store');
const { ApiError } = await import('../../../api/client');
const { rungLockedReason } = await import('../rungLockedReason');
const { TURN_RUNNING_REASON } = await import('@lucidos/engine-constants');
const { getComposeSelectionOverride, clearComposeSelection } = await import('../../../store/composeSelections');
const { makeThreadState } = await import('../../../store/actions/threads-test-helpers');
const { MODELS } = await import('../../../store/models');
const { OFF_LADDER_NOTE } = await import('../rungRows');

/** The models the tests use, read from the fallback list. */
const idOf = (label: string) => MODELS.find((m) => m.label === label)!.value;
const HAIKU = idOf('Haiku 5.5');
const SONNET = idOf('Sonnet 5.5');

beforeAll(() => controlMenuParts.preload());

const REBUILD: SwitchEstimate = { context_tokens: 300_000, rebuilds_cache: 'yes', extra_cost_usd: 0.42 };
const KEEPS_CACHE: SwitchEstimate = { context_tokens: 300_000, rebuilds_cache: 'no', extra_cost_usd: null };

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

function openList(): Promise<void> {
  click('.rung-trigger');
  return settle();
}

/** A row's label, less the checkmark the row in force wears. */
const labelOf = (row: Element | null) => row?.querySelector('.control-option-label')?.textContent?.replace('✓', '');

function rowLabels(): string[] {
  return Array.from(document.querySelectorAll('.control-option')).map((e) => labelOf(e) ?? '');
}

function rowNote(label: string): string | undefined {
  const row = Array.from(document.querySelectorAll('.control-option')).find((e) => labelOf(e) === label);
  return row?.querySelector('.control-option-desc')?.textContent ?? undefined;
}

function checkedRow(): string | undefined {
  return labelOf(document.querySelector('.control-option-current'));
}

/** Pick Haiku's rung from the list: one tap commits model and effort. */
async function pickHaiku(): Promise<void> {
  await openList();
  click('.control-option', 'Haiku 5.5 · low');
  await settle();
}

/** The model the engine was asked to record for `t1`, if any. */
function recordedModel(): string | undefined {
  return recordChatSettings.mock.calls.find(([id]) => id === 't1')?.[1].model;
}

function setStatus(status: 'idle' | 'running' | 'waiting_for_user_answer' | 'paused'): void {
  threadMap.value = new Map([['t1', makeThreadState('t1', { meta: { channel: 'chat', status, state: 'active' } })]]);
}

const LOCKED = rungLockedReason('running')!;

function renderControl(props: { threadId?: string; composeContext: boolean }): void {
  render(<><RungControl {...props} codingAgent={null} /><ConfirmDialog /></>, host);
}

beforeEach(() => {
  vi.clearAllMocks();
  currentModel.value = OPUS;
  // A failed registry read is still an answer: the pill draws from the fallback list.
  chatModels.value = { status: 'failed', error: 'test' };
  threadMap.value = new Map([['t1', makeThreadState('t1', { meta: { channel: 'chat', status: 'idle', state: 'active' } })]]);
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  settleConfirm(false);
  threadMap.value = new Map();
  chatLadder.value = { status: 'not-loaded' };
  chatModels.value = { status: 'not-loaded' };
  clearComposeSelection('d1');
  toasts.value = [];
});

describe('the Lucidos Agent rung control', () => {
  it('names the model on the compose pill, between its steps', async () => {
    renderControl({ threadId: 'd1', composeContext: true });
    await settle();
    expect(host.querySelector('.rung-pill .rung-name')!.textContent).toContain('Opus 5.5');
    expect(host.querySelectorAll('.rung-pill .rung-step')).toHaveLength(2);
  });

  it('draws the same pill, steps and all, once the thread has started', async () => {
    renderControl({ threadId: 't1', composeContext: false });
    await settle();
    expect(host.querySelector('.rung-pill .rung-name')!.textContent).toContain('Opus 5.5');
    expect(host.querySelectorAll('.rung-pill .rung-step')).toHaveLength(2);
  });
});

describe('the shared chat ladder', () => {
  it('is read from the store, so a control mounted after the load starts no fetch', async () => {
    chatLadder.value = { status: 'loaded', data: servedChatLadder() };
    renderControl({ threadId: 'd1', composeContext: true });
    await settle();
    expect(getChatLadder).not.toHaveBeenCalled();
    expect(host.querySelector('.rung-pill .rung-name')!.textContent).toContain('Opus 5.5');
  });

  const pill = () => host.querySelector<HTMLElement>('.rung-pill')!;

  it('keeps the pill unseen until the ladder and the registry have both answered', async () => {
    chatLadder.value = { status: 'loading' };
    chatModels.value = { status: 'loaded', data: [] };
    renderControl({ threadId: 'd1', composeContext: true });
    await settle();
    expect(pill().hasAttribute('data-pending')).toBe(true);
    expect(host.querySelector('.rung-name')).toBeNull();

    chatLadder.value = { status: 'loaded', data: servedChatLadder() };
    chatModels.value = { status: 'loading' };
    await settle();
    expect(pill().hasAttribute('data-pending')).toBe(true);
  });

  it('draws its face on the first render it is seen', async () => {
    chatLadder.value = { status: 'loading' };
    renderControl({ threadId: 'd1', composeContext: true });
    await settle();
    expect(pill().hasAttribute('data-pending')).toBe(true);

    chatLadder.value = { status: 'loaded', data: servedChatLadder() };
    await settle();
    expect(pill().hasAttribute('data-pending')).toBe(false);
    expect(host.querySelector('.rung-name')!.textContent).toContain('Opus 5.5');
  });

  it('still draws the pill when the ladder read failed, and retries once on mount', async () => {
    chatLadder.value = { status: 'failed', error: 'nope' };
    getChatLadder.mockRejectedValue(new Error('still down'));
    renderControl({ threadId: 'd1', composeContext: true });
    await settle();
    expect(getChatLadder).toHaveBeenCalledTimes(1);
    expect(host.querySelector('.rung-pill .rung-name')!.textContent).toContain('Opus 5.5');
  });

  it('recovers the ladder on a mount after a failed read', async () => {
    chatLadder.value = { status: 'failed', error: 'nope' };
    getChatLadder.mockResolvedValue(servedChatLadder());
    renderControl({ threadId: 'd1', composeContext: true });
    await settle();
    expect(chatLadder.value.status).toBe('loaded');
  });
});

describe('a pick on a draft', () => {
  it('is the draft\'s own, takes effect at once, and asks the engine nothing', async () => {
    renderControl({ threadId: 'd1', composeContext: true });
    await settle();
    await pickHaiku();
    expect(fetchSwitchEstimate).not.toHaveBeenCalled();
    expect(confirmState.value.visible).toBe(false);
    expect(getComposeSelectionOverride('d1').model).toBe(HAIKU);
    expect(currentModel.value, 'a compose pick never writes the account default').toBe(OPUS);
  });
});

describe('a pick on a started thread', () => {
  beforeEach(async () => {
    renderControl({ threadId: 't1', composeContext: false });
    await settle();
  });

  it('asks the engine what it costs, and applies it silently when the cache is kept', async () => {
    fetchSwitchEstimate.mockResolvedValue(KEEPS_CACHE);
    await pickHaiku();
    expect(fetchSwitchEstimate).toHaveBeenCalledWith('t1', HAIKU, expect.anything());
    expect(confirmState.value.visible).toBe(false);
    expect(recordedModel()).toBe(HAIKU);
    expect(toasts.value.some(t => t.type === 'success' && t.message.includes('Haiku 5.5'))).toBe(true);
  });

  it('asks first when the pick rebuilds the cache, with the engine figure and no handoff', async () => {
    fetchSwitchEstimate.mockResolvedValue(REBUILD);
    await pickHaiku();
    expect(confirmState.value.visible).toBe(true);
    const dialog = document.querySelector('.confirm-dialog')!.textContent!;
    expect(dialog).toContain('about $0.42');
    expect(dialog).not.toMatch(/slow/i);
    expect(dialog).not.toMatch(/hand off|fresh session/i);
    expect(recordedModel(), 'nothing is picked before an answer').toBeUndefined();
  });

  it('keeps the pair on Keep', async () => {
    fetchSwitchEstimate.mockResolvedValue(REBUILD);
    await pickHaiku();
    click('.confirm-dialog button', 'Keep');
    await settle();
    expect(recordedModel()).toBeUndefined();
  });

  it('keeps the pair on a dismissal', async () => {
    fetchSwitchEstimate.mockResolvedValue(REBUILD);
    await pickHaiku();
    settleConfirm(false);
    await settle();
    expect(recordedModel()).toBeUndefined();
  });

  it('applies the pick on Switch anyway', async () => {
    fetchSwitchEstimate.mockResolvedValue(REBUILD);
    await pickHaiku();
    click('.confirm-dialog button', 'Switch anyway');
    await settle();
    expect(recordedModel()).toBe(HAIKU);
  });

  it('applies nothing and says so when the estimate cannot be read', async () => {
    fetchSwitchEstimate.mockRejectedValue(new Error('engine down'));
    await pickHaiku();
    expect(recordedModel()).toBeUndefined();
    expect(toasts.value.some(t => t.type === 'error' && t.message.includes('engine down'))).toBe(true);
  });

  it('records nothing when a turn starts while the estimate is read', async () => {
    fetchSwitchEstimate.mockImplementation(async () => {
      setStatus('running');
      return KEEPS_CACHE;
    });
    await pickHaiku();
    expect(recordChatSettings).not.toHaveBeenCalled();
    expect(toasts.value.some(t => t.type === 'info' && t.message === LOCKED)).toBe(true);
  });

  it('says why, and claims no switch, when the engine refuses because a turn runs', async () => {
    fetchSwitchEstimate.mockResolvedValue(KEEPS_CACHE);
    recordChatSettings.mockRejectedValueOnce(
      new ApiError(409, 'locked', { error: 'locked', reason: TURN_RUNNING_REASON }),
    );
    await pickHaiku();
    expect(toasts.value.some(t => t.type === 'info' && t.message === LOCKED)).toBe(true);
    expect(toasts.value.some(t => t.type === 'success')).toBe(false);
  });

  it('shows an error when the engine refuses for another reason', async () => {
    fetchSwitchEstimate.mockResolvedValue(KEEPS_CACHE);
    recordChatSettings.mockRejectedValueOnce(new ApiError(400, 'bad effort'));
    await pickHaiku();
    expect(toasts.value.some(t => t.type === 'error' && t.message.includes('bad effort'))).toBe(true);
    expect(toasts.value.some(t => t.type === 'success')).toBe(false);
  });

  it('follows a pick made on another device', async () => {
    const thread = threadMap.value.get('t1')!;
    const events = new Map(thread.events);
    events.set(10, { type: 'ChatSettingsChanged', model: HAIKU, reasoning_effort: 'low' });
    threadMap.value = new Map([['t1', { ...thread, events }]]);
    await settle();
    expect(host.querySelector('.rung-pill .rung-name')!.textContent).toContain('Haiku 5.5');
  });
});

describe('a started thread while a turn runs', () => {
  for (const status of ['running', 'waiting_for_user_answer', 'paused'] as const) {
    it(`keeps the current model disabled while ${status}`, async () => {
      setStatus(status);
      renderControl({ threadId: 't1', composeContext: false });
      await settle();
      const trigger = host.querySelector<HTMLElement>('.rung-trigger')!;
      expect(trigger.getAttribute('aria-disabled')).toBe('true');
      expect(trigger.getAttribute('data-tooltip')).toBe(LOCKED);
      expect(host.querySelector('.rung-name')!.textContent).toContain('Opus 5.5');
      trigger.click();
      trigger.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
      for (const step of host.querySelectorAll<HTMLElement>('.rung-step')) {
        expect(step.getAttribute('aria-disabled')).toBe('true');
        step.click();
      }
      await settle();
      expect(document.querySelector('.control-dropdown')).toBeNull();
      expect(fetchSwitchEstimate).not.toHaveBeenCalled();
      expect(recordChatSettings).not.toHaveBeenCalled();
    });
  }
});

const ALL_EFFORTS = ['low', 'medium', 'high', 'xhigh', 'max'];

/** A registry row served by `providers`, in route order. */
const registryRow = (id: string, label: string, providers = ['vertex']): ModelInfo => ({
  id,
  label,
  routes: providers.map((provider) => ({ provider, id, reasoning_efforts: ALL_EFFORTS })),
  preferred_provider: null,
  vision: false,
  default_effort: 'medium',
  successor: null,
  sort_order: 0,
  source: 'builtin',
  on_chat_ladder: true,
  created_at: '2026-01-01T00:00:00Z',
});

describe('the chat rows', () => {
  beforeEach(() => {
    chatModels.value = {
      status: 'loaded',
      data: [
        registryRow(HAIKU, 'Haiku 5.5', ['vertex', 'anthropic']),
        registryRow(SONNET, 'Sonnet 5.5'),
        registryRow(OPUS, 'Opus 5.5'),
        registryRow('claude-opus-5-5[1m]', 'Opus 5.5 (1M)'),
      ],
    };
  });

  afterEach(() => {
    chatModels.value = { status: 'not-loaded' };
  });

  it('list the chat ladder\'s on rungs and the Intelligence link, last, on a draft', async () => {
    renderControl({ threadId: 'd1', composeContext: true });
    await settle();
    await openList();
    expect(rowLabels()).toEqual([
      'Haiku 5.5 · low', 'Opus 5.5 · medium', 'Opus 5.5 · high', 'Edit the intelligence ladder',
    ]);
    expect(rowNote('Edit the intelligence ladder')).toContain('Settings › Intelligence');
  });

  it('check the default rung while the draft follows the account default', async () => {
    renderControl({ threadId: 'd1', composeContext: true });
    await settle();
    await openList();
    expect(checkedRow()).toBe('Opus 5.5 · medium');
  });

  it('check the rung in force on a started thread', async () => {
    renderControl({ threadId: 't1', composeContext: false });
    await settle();
    await openList();
    expect(checkedRow()).toBe('Opus 5.5 · medium');
  });

  it('add the pair in force when the ladder does not hold it', async () => {
    currentModel.value = SONNET;
    renderControl({ threadId: 't1', composeContext: false });
    await settle();
    await openList();
    const offLadder = checkedRow()!;
    expect(offLadder).toMatch(/^Sonnet 5\.5/);
    expect(rowNote(offLadder)).toBe(OFF_LADDER_NOTE);
    expect(rowLabels().slice(-1)[0]).toBe('Edit the intelligence ladder');
  });

  it('open Settings > Intelligence from the link, and pick nothing', async () => {
    renderControl({ threadId: 'd1', composeContext: true });
    await settle();
    await openList();
    click('.control-option', 'Edit the intelligence ladder');
    await settle();
    expect(openIntelligenceLadder).toHaveBeenCalledWith('chat');
    expect(getComposeSelectionOverride('d1').model).toBeUndefined();
  });

  it('commit a rung\'s model and effort with one tap', async () => {
    renderControl({ threadId: 'd1', composeContext: true });
    await settle();
    await openList();
    click('.control-option', 'Opus 5.5 · high');
    await settle();
    expect(getComposeSelectionOverride('d1')).toMatchObject({ model: OPUS, reasoningEffort: 'high' });
  });

  it('keep the 1M window when the rung picked runs the model in force', async () => {
    currentModel.value = 'claude-opus-5-5[1m]';
    renderControl({ threadId: 'd1', composeContext: true });
    await settle();
    await openList();
    click('.control-option', 'Opus 5.5 · high');
    await settle();
    expect(getComposeSelectionOverride('d1')).toMatchObject({ model: 'claude-opus-5-5[1m]', reasoningEffort: 'high' });
  });

  it('open the preferred-provider step for a rung with a choice of backend', async () => {
    renderControl({ threadId: 'd1', composeContext: true });
    await settle();
    await openList();
    click('.control-option', 'Haiku 5.5 · low');
    await settle();
    expect(getComposeSelectionOverride('d1').model, 'nothing commits before the backend').toBeUndefined();
    expect(document.querySelector('.control-list')!.getAttribute('data-surface-step')).toBe('Haiku 5.5 · Provider');
    click('.control-option', 'Anthropic');
    await settle();
    expect(getComposeSelectionOverride('d1')).toMatchObject({
      model: HAIKU, reasoningEffort: 'low', provider: 'anthropic',
    });
  });

  it('step one rung from the pill, quietly', async () => {
    renderControl({ threadId: 'd1', composeContext: true });
    await settle();
    await openList();
    click('.control-option', 'Opus 5.5 · medium');
    await settle();
    toasts.value = [];
    click('.rung-step[aria-label="Next model and effort"]');
    await settle();
    expect(getComposeSelectionOverride('d1')).toMatchObject({ model: OPUS, reasoningEffort: 'high' });
    expect(toasts.value).toEqual([]);
    expect(host.querySelector('.rung-step[aria-label="Next model and effort"]')!.getAttribute('aria-disabled'))
      .toBe('true');
  });
});
