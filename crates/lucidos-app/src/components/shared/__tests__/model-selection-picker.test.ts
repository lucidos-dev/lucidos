import { describe, it, expect } from 'vitest';
import {
  modelStepCommit, modelStepOptions, pickerFocusTarget, pickerKeyAction,
  pickerShowsFilter, providerStepCommit, providerStepOptions, RECOMMENDED_TIER_BADGE,
  tierStepOptions,
} from '../ModelSelectionPicker';
import { modelRows, type ModelRow } from '../../../store/modelSelection';
import { LUCIDOS_TIER_VOCABULARY } from '../../../store/actions/models';

const OPUS: ModelRow = {
  value: 'claude-opus-5[1m]',
  label: 'Opus 5 (1M)',
  tiers: [
    { value: 'high', label: 'High' },
    { value: 'xhigh', label: 'X-High', description: 'Deeper' },
  ],
  provider: null,
  providerLabel: null,
  providers: [],
};

const HAIKU: ModelRow = {
  value: 'claude-haiku-4-5-20251001',
  label: 'Haiku 4.5',
  description: 'Fast',
  tiers: [{ value: 'low', label: 'Low' }],
  provider: null,
  providerLabel: null,
  providers: [],
};

const IMAGEN: ModelRow = {
  value: 'imagen-4', label: 'Imagen 4', tiers: [], provider: null, providerLabel: null,
  providers: [],
};

const IN_FORCE = { model: 'claude-opus-5[1m]', label: 'Opus 5 (1M) · X-High' };

describe('modelStepOptions', () => {
  it('reads the whole pair on the model in force', () => {
    const rows = modelStepOptions([OPUS, HAIKU], IN_FORCE);
    expect(rows[0].label).toBe('Opus 5 (1M) · X-High');
  });

  it('reads a bare name on every other model', () => {
    // Printing a tier on all thirty is the flat list this step replaced.
    const rows = modelStepOptions([OPUS, HAIKU], IN_FORCE);
    expect(rows[1].label).toBe('Haiku 4.5');
  });

  it('carries the model id, not an encoded pair: a model is not a selection', () => {
    expect(modelStepOptions([HAIKU], IN_FORCE)[0].value).toBe('claude-haiku-4-5-20251001');
  });

  it('marks a model with tiers as opening another list', () => {
    const rows = modelStepOptions([OPUS, IMAGEN], IN_FORCE);
    expect(rows[0].drilldown).toBe(true);
    expect(rows[1].drilldown).toBe(false);
  });

  it('shows the model description, and lets a host override it', () => {
    expect(modelStepOptions([HAIKU], IN_FORCE)[0].description).toBe('Fast');
    expect(modelStepOptions([HAIKU], IN_FORCE, () => 'Currently Opus 5')[0].description)
      .toBe('Currently Opus 5');
  });

  it('reads the pair on a tierless model in force, which is its name alone', () => {
    const rows = modelStepOptions([IMAGEN], { model: 'imagen-4', label: 'Imagen 4' });
    expect(rows[0].label).toBe('Imagen 4');
  });
});

describe('tierStepOptions', () => {
  it('carries the encoded pair on every row, so one choice sets both halves', () => {
    expect(tierStepOptions(OPUS).map((o) => o.value))
      .toEqual(['claude-opus-5[1m]|high', 'claude-opus-5[1m]|xhigh']);
  });

  it('labels a row with the tier alone: the step already names the model', () => {
    expect(tierStepOptions(OPUS).map((o) => o.label)).toEqual(['High', 'X-High']);
  });

  it('keeps the tier description the surface vocabulary supplied', () => {
    expect(tierStepOptions(OPUS)[1].description).toBe('Deeper');
  });

  it('is empty for a model with no tiers, so there is no step to open', () => {
    expect(tierStepOptions(IMAGEN)).toEqual([]);
  });

  it('leads with a Default row committing no tier, when the surface names one', () => {
    const rows = tierStepOptions(OPUS, () => 'Default (High)');
    expect(rows[0]).toEqual({ value: 'claude-opus-5[1m]|', label: 'Default (High)' });
    expect(rows.slice(1).map((o) => o.label)).toEqual(['High', 'X-High']);
    expect(tierStepOptions(IMAGEN, () => 'Default')).toEqual([]);
  });

  /** The recommended tier wears the badge; a model with none badges nothing. */
  it('badges the tier the model choice recommends, and only that one', () => {
    const choice = { value: 'gpt-6.1-sol', label: 'GPT-6.1 Sol', reasoningEfforts: ['low', 'medium', 'high'] };
    const badges = (recommendedEffort?: string) => {
      const [row] = modelRows([{ ...choice, recommendedEffort }], LUCIDOS_TIER_VOCABULARY);
      return tierStepOptions(row).map((o) => [o.label, o.badge]);
    };
    expect(badges('low')).toEqual([
      ['Low', RECOMMENDED_TIER_BADGE],
      ['Med', undefined],
      ['High', undefined],
    ]);
    expect(badges(undefined).every(([, badge]) => badge === undefined)).toBe(true);
  });
});

describe('modelStepCommit', () => {
  it('reports nothing for a model with tiers, so backing out changes nothing', () => {
    expect(modelStepCommit(OPUS)).toBeNull();
  });

  it('commits a tierless model whole, with no effort at all', () => {
    expect(modelStepCommit(IMAGEN)).toBe('imagen-4|');
  });
});

describe('pickerKeyAction', () => {
  it('takes Enter and the arrows', () => {
    expect(pickerKeyAction('Enter')).toBe('choose');
    expect(pickerKeyAction('ArrowDown')).toBe('next');
    expect(pickerKeyAction('ArrowUp')).toBe('prev');
  });

  it('never takes Escape, which belongs to the overlay stack', () => {
    // The Escape dispatcher runs in the capture phase and stops propagation,
    // so a keydown handler here would never see the key. Stepping back from
    // the tier list is a stack registrant instead.
    expect(pickerKeyAction('Escape')).toBeNull();
  });

  it('leaves every other key alone, so typing reaches the filter box', () => {
    expect(pickerKeyAction('o')).toBeNull();
    expect(pickerKeyAction('Tab')).toBeNull();
  });
});

describe('pickerShowsFilter', () => {
  it('draws no box on a freshly opened desktop panel', () => {
    // The user opened it to click a row. A caret blinking in an empty box is
    // the thing this replaced.
    expect(pickerShowsFilter({ searching: false, touch: false, keyboard: false })).toBe(false);
  });

  it('draws it once a keystroke has started the search', () => {
    expect(pickerShowsFilter({ searching: true, touch: false, keyboard: false })).toBe(true);
  });

  it('always draws it on a touch device, where no keystroke can reveal it', () => {
    // The capability, never the mobile width breakpoint. A phone in landscape
    // is wider than 768px, and has no more keyboard than it had upright. A
    // width test would leave it a box it can never summon.
    expect(pickerShowsFilter({ searching: false, touch: true, keyboard: false })).toBe(true);
  });

  it('draws it for a host opened mid-typing, so the keystrokes have a home', () => {
    // A phone-width window under a mouse is no touch device, yet the prompt
    // held focus as the menu opened.
    expect(pickerShowsFilter({ searching: false, touch: false, keyboard: true })).toBe(true);
  });
});

describe('pickerFocusTarget', () => {
  const target = (over: Partial<Parameters<typeof pickerFocusTarget>[0]> = {}) =>
    pickerFocusTarget({ tierStep: false, searching: false, touch: false, keyboard: false, ...over });

  it('gives the list the keystrokes before the search starts', () => {
    // The list's own handler turns a printable key into the query, so it holds
    // focus while there is no box to type into.
    expect(target()).toBe('list');
  });

  it('hands over to the box once the search has started', () => {
    expect(target({ searching: true })).toBe('filter');
  });

  it('keeps the tier step on the list, which has no filter to focus', () => {
    expect(target({ tierStep: true, searching: true })).toBe('list');
  });

  it('leaves focus alone on a touch device with no keyboard up', () => {
    // Focusing the list takes focus from the prompt, and on iOS the keyboard
    // then slides away under the open panel.
    expect(target({ touch: true })).toBeNull();
    expect(target({ touch: true, tierStep: true })).toBeNull();
  });

  it('gives the box the keyboard a touch host opened mid-typing', () => {
    // Typing then filters the models instead of going into the prompt.
    expect(target({ touch: true, keyboard: true })).toBe('filter');
  });

  it('gives the box focus for any host opened mid-typing, a mouse one included', () => {
    // A phone-width window leaves the prompt focused under a mouse too.
    expect(target({ keyboard: true })).toBe('filter');
    expect(target({ keyboard: true, tierStep: true })).toBe('list');
  });

  it('hands the keyboard back to its holder on the tier step, which has no box', () => {
    // Focus left on nothing would drop the keyboard, and the panel would jump.
    expect(target({ touch: true, keyboard: true, tierStep: true })).toBe('holder');
  });
});

/** A row with a real choice of backend. OpenRouter has no `xhigh`. */
const DUAL: ModelRow = {
  value: 'claude-opus-5-5',
  label: 'Opus 5.5',
  tiers: [
    { value: 'high', label: 'High' },
    { value: 'xhigh', label: 'X-High' },
  ],
  provider: 'vertex',
  providerLabel: 'Vertex',
  providers: [
    {
      value: 'vertex', label: 'Vertex', configured: true,
      tiers: [{ value: 'high', label: 'High' }, { value: 'xhigh', label: 'X-High' }],
    },
    { value: 'openrouter', label: 'OpenRouter', configured: false, tiers: [{ value: 'high', label: 'High' }] },
  ],
};

describe('the provider step', () => {
  it('marks a backend that is not set up, since picking it refuses the turn', () => {
    expect(providerStepOptions(DUAL).map((o) => [o.value, o.description])).toEqual([
      ['vertex', undefined],
      ['openrouter', 'Not set up'],
    ]);
  });

  it('snaps the held effort onto what the picked backend accepts', () => {
    expect(providerStepCommit(DUAL, 'openrouter', 'xhigh')).toBe('claude-opus-5-5|high');
    expect(providerStepCommit(DUAL, 'vertex', 'xhigh')).toBe('claude-opus-5-5|xhigh');
  });

  it('opens a step rather than committing, for a model with a choice of backend', () => {
    expect(modelStepCommit({ ...DUAL, tiers: [] })).toBeNull();
    expect(modelStepOptions([DUAL], IN_FORCE)[0].drilldown).toBe(true);
  });
});
