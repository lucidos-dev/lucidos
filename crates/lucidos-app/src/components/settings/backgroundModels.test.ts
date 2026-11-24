import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import type { BackgroundModel, ModelInfo } from '../../api/types';
import { chatModels, configuredProviders } from '../../store/store';
import {
  backgroundModelChoices,
  backgroundRowCaveat,
  OTHER_MODELS_SECTION,
  RECOMMENDED_SECTION,
} from './backgroundModels';

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
    successor: null,
    sort_order: 0,
    source: 'builtin',
    on_chat_ladder: true,
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

  it('keeps the recommended order and lists each model once', () => {
    configuredProviders.value = ['anthropic', 'vertex'];
    const choices = backgroundModelChoices(untiered('cheap-anthropic-model', 'cheap-vertex-model'), null);
    expect(choices.map((c) => c.value)).toEqual([
      'cheap-anthropic-model',
      'cheap-vertex-model',
      'flagship-model',
    ]);
  });

  it('draws no sections when no recommended model is offered', () => {
    const choices = backgroundModelChoices([], null);
    expect(choices.every((c) => c.section === undefined)).toBe(true);
    expect(choices.map((c) => c.value)).toEqual(['flagship-model', 'cheap-anthropic-model']);
  });

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
    expect(backgroundModelChoices(untiered('cheap-anthropic-model'), null).map((c) => c.value)).toEqual([
      'cheap-anthropic-model',
      'flagship-model',
    ]);
  });

  it('keeps a stored text-only pick on a task that needs vision', () => {
    const choices = backgroundModelChoices([], 'cheap-anthropic-model', true);
    expect(choices.map((c) => c.value)).toEqual(['cheap-anthropic-model']);
  });

  it('keeps a stored pick the registry does not offer', () => {
    const choices = backgroundModelChoices(untiered('cheap-anthropic-model'), 'cheap-vertex-model');
    expect(choices.map((c) => c.value)).toContain('cheap-vertex-model');
  });

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
    generation: null,
    not_served: [],
    needs_vision: false,
    asks_no_model: false,
    unresolved: null,
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

  it('names an unreachable model before an image-blind one', () => {
    expect(backgroundRowCaveat(row(false, { needs_vision: true }), null)).toBe(
      'No configured provider serves this model',
    );
  });

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

  it('names the successor a retired pick runs', () => {
    expect(
      backgroundRowCaveat(
        row(true, { generation: { kind: 'successor', model: 'newer-model' } }),
        null,
      ),
    ).toBe('Retired, so this runs its successor newer-model');
  });

  it('names the earlier version a refused pick runs', () => {
    expect(
      backgroundRowCaveat(
        row(true, {
          model: 'newer-model',
          not_served: ['newer-model'],
          generation: { kind: 'predecessor', model: 'older-model' },
        }),
        null,
      ),
    ).toBe('newer-model is not served here, so this runs older-model, an earlier version');
  });

  it('keeps what a default moved past beside the generation it runs', () => {
    expect(
      backgroundRowCaveat(
        row(true, {
          source: 'default',
          not_served: ['refused-model'],
          generation: { kind: 'successor', model: 'newer-model' },
        }),
        null,
      ),
    ).toBe(
      'Retired, so this runs its successor newer-model. Moved past refused-model: its provider no longer serves it',
    );
  });

  it('names the successor a held retired pick is waiting on', () => {
    expect(
      backgroundRowCaveat(
        row(true, {
          model: 'older-model',
          generation: { kind: 'held', successor: 'newer-model' },
        }),
        null,
      ),
    ).toBe('Retired, but newer-model is not served here, so this keeps running older-model');
  });

  it('says when no measured model is reachable', () => {
    expect(
      backgroundRowCaveat(row(true, { source: 'chat-model', asks_no_model: true }), null),
    ).toBe('No measured model is reachable, so every report wakes its parent');
    expect(
      backgroundRowCaveat(row(false, { source: 'chat-model', asks_no_model: true }), null),
      'a model the gate never asks cannot be the problem',
    ).toBe('No measured model is reachable, so every report wakes its parent');
  });

  it('says when the engine could not be asked', () => {
    expect(backgroundRowCaveat(null, 'HTTP 500')).toBe('Could not read the default model: HTTP 500');
  });
});
