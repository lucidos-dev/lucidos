import { describe, it, expect } from 'vitest';
import {
  modelStepCommit, modelStepOptions, pickerKeyAction, tierStepOptions,
} from '../ModelSelectionPicker';
import type { ModelRow } from '../../../store/modelSelection';

const OPUS: ModelRow = {
  value: 'claude-opus-5',
  label: 'Opus 5 (1M)',
  tiers: [
    { value: 'high', label: 'High' },
    { value: 'xhigh', label: 'X-High', description: 'Deeper' },
  ],
};

const HAIKU: ModelRow = {
  value: 'claude-haiku-4-5',
  label: 'Haiku 4.5',
  description: 'Fast',
  tiers: [{ value: 'low', label: 'Low' }],
};

const IMAGEN: ModelRow = { value: 'imagen-4', label: 'Imagen 4', tiers: [] };

const IN_FORCE = { model: 'claude-opus-5', label: 'Opus 5 (1M) · X-High' };

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
    expect(modelStepOptions([HAIKU], IN_FORCE)[0].value).toBe('claude-haiku-4-5');
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
      .toEqual(['claude-opus-5|high', 'claude-opus-5|xhigh']);
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
