import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import type { BackgroundModel, ModelInfo } from '../../api/types';
import { chatModels, configuredProviders } from '../../store/store';
import {
  backgroundModelChoices,
  backgroundRowCaveat,
  OTHER_MODELS_SECTION,
  RECOMMENDED_SECTION,
} from './backgroundModels';

/** Recommended models with no recommended tier, as every row but the
 *  compactor's sends them. */
function untiered(...models: string[]) {
  return models.map((model) => ({ model, effort: null }));
}

function registryRow(id: string, provider: string, vision = false): ModelInfo {
  return {
    id,
    label: id,
    routes: [{ provider, id, reasoning_efforts: ['none', 'low'] }],
    preferred_provider: null,
    vision,
    default_effort: null,
    sort_order: 0,
    source: 'builtin',
    enabled: true,
    created_at: '2026-01-01T00:00:00Z',
  };
}

describe('backgroundModelChoices', () => {
  beforeEach(() => {
    chatModels.value = {
      status: 'loaded',
      data: [
        registryRow('flagship-model', 'anthropic'),
        registryRow('cheap-vertex-model', 'vertex'),
        registryRow('cheap-anthropic-model', 'anthropic'),
      ],
    };
    configuredProviders.value = ['anthropic'];
  });

  afterEach(() => {
    chatModels.value = { status: 'not-loaded' };
    configuredProviders.value = null;
  });

  /** Every model the chat picker offers, the reachable recommended ones first
   *  in their order. The Vertex model is recommended, but nothing configured serves it. */
  it('offers the chat picker list, recommended first, unreachable ones left out', () => {
    const choices = backgroundModelChoices(
      untiered('cheap-vertex-model', 'cheap-anthropic-model'),
      null,
    );
    expect(choices.map((c) => [c.value, c.section])).toEqual([
      ['cheap-anthropic-model', RECOMMENDED_SECTION],
      ['flagship-model', OTHER_MODELS_SECTION],
    ]);
  });

  /** The recommended order wins over the registry's, and no model repeats. */
  it('keeps the recommended order and lists each model once', () => {
    configuredProviders.value = ['anthropic', 'vertex'];
    const choices = backgroundModelChoices(untiered('cheap-anthropic-model', 'cheap-vertex-model'), null);
    expect(choices.map((c) => c.value)).toEqual([
      'cheap-anthropic-model',
      'cheap-vertex-model',
      'flagship-model',
    ]);
  });

  /** With nothing recommended to show, a lone "Other models" heading would
   *  name a section with no sibling, so the list has none. */
  it('draws no sections when no recommended model is offered', () => {
    const choices = backgroundModelChoices([], null);
    expect(choices.every((c) => c.section === undefined)).toBe(true);
    expect(choices.map((c) => c.value)).toEqual(['flagship-model', 'cheap-anthropic-model']);
  });

  /** A task that sends images offers only models that read them. */
  it('offers only image-reading models when the task needs vision', () => {
    chatModels.value = {
      status: 'loaded',
      data: [
        registryRow('flagship-model', 'anthropic', true),
        registryRow('cheap-anthropic-model', 'anthropic'),
      ],
    };
    const choices = backgroundModelChoices(untiered('cheap-anthropic-model'), null, true);
    expect(choices.map((c) => c.value)).toEqual(['flagship-model']);
    // Other rows are unchanged.
    expect(backgroundModelChoices(untiered('cheap-anthropic-model'), null).map((c) => c.value)).toEqual([
      'cheap-anthropic-model',
      'flagship-model',
    ]);
  });

  /** A stored pick that cannot read images stays listed, so the row shows
   *  what is set, beside its caveat. */
  it('keeps a stored text-only pick on a task that needs vision', () => {
    const choices = backgroundModelChoices([], 'cheap-anthropic-model', true);
    expect(choices.map((c) => c.value)).toEqual(['cheap-anthropic-model']);
  });

  /** A stored pick nothing serves still renders as selected. */
  it('keeps a stored pick the registry does not offer', () => {
    const choices = backgroundModelChoices(untiered('cheap-anthropic-model'), 'cheap-vertex-model');
    expect(choices.map((c) => c.value)).toContain('cheap-vertex-model');
  });

  /** The engine names a recommended tier only on the compactor's row. A model
   *  carries the tier it was sent, and no other model carries one. */
  it('carries the recommended tier the engine sends, and only that', () => {
    const choices = backgroundModelChoices(
      [{ model: 'cheap-anthropic-model', effort: 'low' }],
      null,
    );
    expect(choices.map((c) => [c.value, c.recommendedEffort])).toEqual([
      ['cheap-anthropic-model', 'low'],
      ['flagship-model', undefined],
    ]);
    expect(backgroundModelChoices(untiered('cheap-anthropic-model'), null)[0].recommendedEffort)
      .toBeUndefined();
  });
});

describe('backgroundRowCaveat', () => {
  const row = (reachable: boolean, more: Partial<BackgroundModel> = {}): BackgroundModel => ({
    model: 'cheap-vertex-model',
    effort: 'none',
    source: 'preference',
    reachable,
    not_served: [],
    needs_vision: false,
    vision: false,
    recommended: [],
    ...more,
  });

  it('says nothing for a reachable model', () => {
    expect(backgroundRowCaveat(row(true), null)).toBeNull();
  });

  it('says when no configured provider serves the model', () => {
    expect(backgroundRowCaveat(row(false), null)).toBe('No configured provider serves this model');
  });

  it('says when the chosen model cannot read images', () => {
    expect(backgroundRowCaveat(row(true, { needs_vision: true }), null)).toBe(
      'This model cannot read images, so images go undescribed',
    );
  });

  it('says when no recommended model that reads images is reachable', () => {
    expect(
      backgroundRowCaveat(row(true, { needs_vision: true, source: 'chat-model' }), null),
    ).toBe('No recommended model that reads images is reachable. Pick one that does');
  });

  it('says nothing about images for a task that sends none', () => {
    expect(backgroundRowCaveat(row(true), null)).toBeNull();
    expect(backgroundRowCaveat(row(true, { needs_vision: true, vision: true }), null)).toBeNull();
  });

  /** An unreachable model is the louder problem, so it wins. */
  it('names an unreachable model before an image-blind one', () => {
    expect(backgroundRowCaveat(row(false, { needs_vision: true }), null)).toBe(
      'No configured provider serves this model',
    );
  });

  /** A provider refusing the pick is not a missing credential, and adding
   *  one would not help, so it gets its own sentence (ADR 0403). */
  it('says when the provider no longer serves the pick', () => {
    expect(
      backgroundRowCaveat(row(false, { not_served: ['cheap-vertex-model'] }), null),
    ).toBe('Its provider no longer serves this model. Pick another');
  });

  it('names a default it moved past', () => {
    expect(
      backgroundRowCaveat(row(true, { source: 'default', not_served: ['claude-haiku-4-5'] }), null),
    ).toBe('Moved past claude-haiku-4-5: its provider no longer serves it');
  });

  it('says when the engine could not be asked', () => {
    expect(backgroundRowCaveat(null, 'HTTP 500')).toBe('Could not read the default model: HTTP 500');
  });
});
