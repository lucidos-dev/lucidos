// @vitest-environment jsdom
/**
 * The Lucidos Agent's rung control: the same split button and pill as a coding
 * agent's, listing the chat ladder's rungs. A draft's pick is the draft's own.
 * A pick on a started thread asks the engine whether it rebuilds a warm cache,
 * and asks the user before it does. A chat thread has no successor, so the
 * choice is Keep or Switch anyway.
 */
import { beforeAll, describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import type { SwitchEstimate } from '../../../api/client';
import type { IntelligenceLadder, ModelInfo, ServedIntelligenceRung } from '../../../api/types';

import { DEFAULT_CHAT_MODEL } from '../../../store/models';

/** The default chat model is Opus 5.5, whose label the pill reads from the
 *  fallback model list. */
const OPUS = DEFAULT_CHAT_MODEL;

const fetchSwitchEstimate = vi.fn<() => Promise<SwitchEstimate>>();

const served = (model: string, label: string, effort: string, on = true): ServedIntelligenceRung => ({
  model, label, effort, on, estimated: true, intelligence: null, cost: null, time: null, is_default: false,
});

/** The chat ladder: Haiku low and Opus medium and high on, Sonnet off. Read
 *  at call time, once the fallback model list has loaded. */
const chatLadder = (): IntelligenceLadder => ({
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

const getChatLadder = vi.fn(async () => chatLadder());

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  fetchSwitchEstimate,
  getChatLadder,
}));
const openSettingsSubview = vi.fn();
vi.mock('../../../store/actions/menu', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/menu')>()),
  openSettingsSubview,
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
  confirmState, settleConfirm, threadMap, toasts, currentModel, settingsScrollTarget, chatModels,
} = await import('../../../store/store');
const { getThreadModelOverride, _resetThreadModelSelectionsForTesting } = await import('../../../store/threadModelSelections');
const { getComposeSelectionOverride, clearComposeSelection } = await import('../../../store/composeSelections');
const { makeThreadState } = await import('../../../store/actions/threads-test-helpers');
const { MODELS } = await import('../../../store/models');
const { OFF_LADDER_NOTE } = await import('../rungRows');

/** The models the tests use, read from the fallback list. */
const idOf = (label: string) => MODELS.find((m) => m.label === label)!.value;
const HAIKU = idOf('Haiku 5.5');
const SONNET = idOf('Sonnet 5.5');

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

function renderControl(props: { threadId?: string; composeContext: boolean }): void {
  render(<><RungControl {...props} codingAgent={null} /><ConfirmDialog /></>, host);
}

beforeEach(() => {
  vi.clearAllMocks();
  currentModel.value = OPUS;
  threadMap.value = new Map([['t1', makeThreadState('t1', { meta: { channel: 'chat', status: 'idle', state: 'active' } })]]);
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  settleConfirm(false);
  threadMap.value = new Map();
  _resetThreadModelSelectionsForTesting();
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
    expect(getThreadModelOverride('t1').model).toBe(HAIKU);
  });

  it('asks first when the pick rebuilds the cache, with the engine figure and no handoff', async () => {
    fetchSwitchEstimate.mockResolvedValue(REBUILD);
    await pickHaiku();
    expect(confirmState.value.visible).toBe(true);
    const dialog = document.querySelector('.confirm-dialog')!.textContent!;
    expect(dialog).toContain('about $0.42');
    expect(dialog).not.toMatch(/slow/i);
    expect(dialog).not.toMatch(/hand off|new thread/i);
    expect(getThreadModelOverride('t1').model, 'nothing is picked before an answer').toBeUndefined();
  });

  it('keeps the pair on Keep', async () => {
    fetchSwitchEstimate.mockResolvedValue(REBUILD);
    await pickHaiku();
    click('.confirm-dialog button', 'Keep');
    await settle();
    expect(getThreadModelOverride('t1').model).toBeUndefined();
  });

  it('keeps the pair on a dismissal', async () => {
    fetchSwitchEstimate.mockResolvedValue(REBUILD);
    await pickHaiku();
    settleConfirm(false);
    await settle();
    expect(getThreadModelOverride('t1').model).toBeUndefined();
  });

  it('applies the pick on Switch anyway', async () => {
    fetchSwitchEstimate.mockResolvedValue(REBUILD);
    await pickHaiku();
    click('.confirm-dialog button', 'Switch anyway');
    await settle();
    expect(getThreadModelOverride('t1').model).toBe(HAIKU);
  });

  it('applies nothing and says so when the estimate cannot be read', async () => {
    fetchSwitchEstimate.mockRejectedValue(new Error('engine down'));
    await pickHaiku();
    expect(getThreadModelOverride('t1').model).toBeUndefined();
    expect(toasts.value.some(t => t.type === 'error' && t.message.includes('engine down'))).toBe(true);
  });
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
    expect(openSettingsSubview).toHaveBeenCalledWith('intelligence');
    expect(settingsScrollTarget.value).toBe('intelligence:ladder-chat');
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
