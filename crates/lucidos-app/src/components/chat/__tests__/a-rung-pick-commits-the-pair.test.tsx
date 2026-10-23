// @vitest-environment jsdom
/**
 * The prompt bar's rung control opens the rung list. One tap on a rung commits
 * its model and effort together, into the draft's own override and no account
 * preference. Another draft keeps its own pick.
 */
import { beforeAll, describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import type { CodingAgentCommandsResponse } from '../../../api/client';
import { composeSelections, getComposeSelectionOverride } from '../../../store/composeSelections';

const response: CodingAgentCommandsResponse = {
  control_commands: [{
    subtype: 'set_model',
    label: 'Model',
    params: [{
      key: 'model',
      options: [
        { value: 'default', label: 'Default (recommended)', description: '' },
        { value: 'claude-opus-5-6', label: 'Opus 5.6', description: '', reasoning_efforts: ['medium', 'high'] },
        { value: 'claude-opus-5-6[1m]', label: 'Opus 5.6 (1M)', description: '', reasoning_efforts: ['medium', 'high'] },
        { value: 'claude-opus-4-8', label: 'Opus 4.8', description: '', reasoning_efforts: ['medium', 'high'] },
      ],
    }],
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
  current_model: null,
  current_reasoning_effort: null,
  has_active_session: false,
  default_implementer: { model: 'claude-opus-5-6', effort: 'medium' },
  ladder_rungs: [
    { model: 'claude-opus-5-6', effort: 'medium', label: 'Opus 5.6', estimated: true, intelligence: { multiple: 1, label: '1.0×' } },
    { model: 'claude-opus-5-6', effort: 'high', label: 'Opus 5.6', estimated: true, intelligence: { multiple: 1.08, label: '1.08×' } },
  ],
} as unknown as CodingAgentCommandsResponse;

const { setChatModelSelection, openIntelligenceLadder } = vi.hoisted(() => ({
  setChatModelSelection: vi.fn(),
  openIntelligenceLadder: vi.fn(),
}));

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  fetchCodingAgentCommands: vi.fn(() => Promise.resolve(response)),
}));
vi.mock('../../../store/actions/preferences', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/preferences')>()),
  setChatModelSelection,
}));

vi.mock('../../../store/actions/menu', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/menu')>()),
  openIntelligenceLadder,
}));

const { RungControl } = await import('../RungControl');
const { controlMenuParts } = await import('../controlMenuParts');

beforeAll(() => controlMenuParts.preload());

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

beforeEach(() => {
  vi.stubGlobal('fetch', vi.fn(async () => new Response('{}', { status: 200 })));
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  composeSelections.value = new Map();
  vi.unstubAllGlobals();
});

async function openRungList(draft: string): Promise<void> {
  render(<RungControl composeThreadId={draft} codingAgent="claude-code" />, host);
  await settle();
  click('.rung-trigger');
  await settle();
}

/** The compose pill's two steps, ‹ first. */
function stepButtons(): HTMLButtonElement[] {
  return Array.from(document.querySelectorAll<HTMLButtonElement>('.rung-step'));
}

describe('the rung list', () => {
  it('offers the ladder rungs and the ladder link, checks the default rung, and lists no old model or 1M twin', async () => {
    await openRungList('d1');
    const labels = Array.from(document.querySelectorAll('.control-option-label')).map(e => e.textContent?.trim());
    expect(labels).toEqual(['✓Opus 5.6 · medium', 'Opus 5.6 · high', 'Edit the intelligence ladder']);
    expect(document.body.textContent).not.toMatch(/Default|Opus 4|1M|recommended/);
  });

  it("commits a rung's model and effort together into this draft only", async () => {
    await openRungList('d1');
    click('.control-option', 'Opus 5.6 · high');
    await settle();
    expect(getComposeSelectionOverride('d1')).toMatchObject({ ccModel: 'claude-opus-5-6', ccReasoningEffort: 'high' });
    expect(getComposeSelectionOverride('d2').ccModel ?? null).toBeNull();
    expect(getComposeSelectionOverride('d2').ccReasoningEffort ?? null).toBeNull();
    expect(setChatModelSelection).not.toHaveBeenCalled();
  });

  it('keeps two drafts on their own picks', async () => {
    await openRungList('d1');
    click('.control-option', 'Opus 5.6 · high');
    await settle();
    render(null, host);
    await openRungList('d2');
    click('.control-option', 'Opus 5.6 · medium');
    await settle();
    expect(getComposeSelectionOverride('d1')).toMatchObject({ ccModel: 'claude-opus-5-6', ccReasoningEffort: 'high' });
    expect(getComposeSelectionOverride('d2')).toMatchObject({ ccModel: 'claude-opus-5-6', ccReasoningEffort: 'medium' });
  });

  it('opens the ladder editor from its link, and picks nothing', async () => {
    await openRungList('d1');
    click('.control-option', 'Edit the intelligence ladder');
    await settle();
    expect(openIntelligenceLadder).toHaveBeenCalledWith('claude-code');
    expect(getComposeSelectionOverride('d1').ccModel ?? null).toBeNull();
  });

  it('steps one row from the pill, into this draft only, with no toast', async () => {
    render(<RungControl composeThreadId="d1" codingAgent="claude-code" />, host);
    await settle();
    // Nothing is picked, so the default rung is in force and only the › step is live.
    expect(stepButtons().map((b) => b.getAttribute('aria-disabled'))).toEqual(['true', 'false']);
    stepButtons()[1].click();
    await settle();
    expect(getComposeSelectionOverride('d1')).toMatchObject({ ccModel: 'claude-opus-5-6', ccReasoningEffort: 'high' });
    expect(getComposeSelectionOverride('d2').ccModel ?? null).toBeNull();
    expect(document.body.textContent).not.toContain('Model:');
    expect(stepButtons().map((b) => b.getAttribute('aria-disabled'))).toEqual(['false', 'true']);
    stepButtons()[0].click();
    await settle();
    expect(getComposeSelectionOverride('d1')).toMatchObject({ ccReasoningEffort: 'medium' });
  });

  it('names the rung in the pill as it steps', async () => {
    render(<RungControl composeThreadId="d1" codingAgent="claude-code" />, host);
    await settle();
    // Nothing picked runs the ladder's default, which is the first rung.
    // The face in force, not the hidden faces and bands reserved behind it.
    const face = () => {
      const shown = host.querySelector('.rung-label > .rung-face:not(.rung-face-reserve)')!.cloneNode(true) as Element;
      shown.querySelectorAll('.rung-intelligence-reserve').forEach((r) => r.remove());
      return shown.textContent;
    };
    expect(face()).toBe('1.0×Opus 5.6 · medium');
    stepButtons()[1].click();
    await settle();
    expect(face()).toBe('1.08×Opus 5.6 · high');
  });
});
