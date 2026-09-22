import { describe, it, expect } from 'vitest';
import { availableReasoningLevels } from '../models';

// The no-`supported` cases below exercise the ID-SHAPE HEURISTIC, which is now
// only the fallback for a model the registry cannot answer for (the picker
// rendering before /models lands, or a saved chat_model with no row). They are
// deliberately unchanged: the heuristic's verdicts are what a pre-load picker
// still shows. The registry's answer overriding them is the block after.
describe('availableReasoningLevels', () => {
  it('drops max for pre-5.6 OpenAI models (collapses with xhigh)', () => {
    const values = availableReasoningLevels('gpt-5.4').map(l => l.value);
    expect(values).toEqual(['none', 'low', 'medium', 'high', 'xhigh']);
  });

  it('exposes full set (incl max) for the GPT-5.6 family and GPT-6 Astra', () => {
    for (const model of ['gpt-5.6-sol', 'gpt-5.6-terra', 'gpt-5.6-luna', 'gpt-6-astra']) {
      const values = availableReasoningLevels(model).map(l => l.value);
      expect(values).toEqual(['none', 'low', 'medium', 'high', 'xhigh', 'max']);
    }
  });

  // Astra is matched by name. A `gpt-6` prefix rule would offer `max` to a
  // later GPT-6 model that rejects it, which is a 400 rather than a downgrade.
  it('does not extend max to every gpt-6 id', () => {
    const values = availableReasoningLevels('gpt-6-mini').map(l => l.value);
    expect(values).toEqual(['none', 'low', 'medium', 'high', 'xhigh']);
  });

  it('exposes full set for Opus 4.7', () => {
    const values = availableReasoningLevels('claude-opus-4-7').map(l => l.value);
    expect(values).toEqual(['none', 'low', 'medium', 'high', 'xhigh', 'max']);
  });

  it('exposes full set for Opus 4.7 with 1M suffix', () => {
    const values = availableReasoningLevels('claude-opus-4-7[1m]').map(l => l.value);
    expect(values).toContain('xhigh');
  });

  // The branch matches the Fable family by prefix, so a new generation is
  // offered the full ladder without a new rule. Fable 5.1 accepts every tier.
  it('exposes full set for both Fable generations', () => {
    for (const id of ['claude-fable-5', 'claude-fable-5-1', 'claude-fable-5-1[1m]']) {
      const values = availableReasoningLevels(id).map(l => l.value);
      expect(values).toEqual(['none', 'low', 'medium', 'high', 'xhigh', 'max']);
    }
  });

  // Same prefix rule on the Opus side: `claude-opus-5` matches Opus 5.5 too,
  // so the point release inherits the full ladder without a new branch.
  it('exposes full set for both Opus 5 generations', () => {
    for (const id of [
      'claude-opus-5@default',
      'claude-opus-5-5',
      'claude-opus-5-5[1m]',
    ]) {
      const values = availableReasoningLevels(id).map(l => l.value);
      expect(values).toEqual(['none', 'low', 'medium', 'high', 'xhigh', 'max']);
    }
  });

  it('drops xhigh for non-Opus-4.7 Claude models', () => {
    const values = availableReasoningLevels('claude-opus-4-6').map(l => l.value);
    expect(values).toEqual(['none', 'low', 'medium', 'high', 'max']);
  });

  it('drops xhigh for Gemini', () => {
    const values = availableReasoningLevels('gemini-3.1-pro-preview').map(l => l.value);
    expect(values).not.toContain('xhigh');
    expect(values).toContain('max');
  });
});

// The registry's answer (a model's `reasoning_efforts` from /models, derived by
// `llm::reasoning::supported_efforts`) beats the heuristic. This is the fix for
// the local-model 400: the heuristic offered `max` because the id matched no
// branch, the engine sent a tier the local server rejected, and the two only
// disagreed because each derived the answer itself.
describe('availableReasoningLevels with the registry answer', () => {
  it('uses the supplied set instead of the id-shape guess', () => {
    const values = availableReasoningLevels('muse-glimmer:30b-mlx', [
      'none', 'low', 'medium', 'high',
    ]).map(l => l.value);
    expect(values).toEqual(['none', 'low', 'medium', 'high']);
  });

  it('can narrow a model the heuristic would have been generous with', () => {
    // The heuristic hands any unknown id everything but xhigh, `max` included.
    expect(availableReasoningLevels('z-ai/glm-5.2').map(l => l.value)).toContain('max');
    const registry = availableReasoningLevels('z-ai/glm-5.2', [
      'none', 'low', 'medium', 'high',
    ]).map(l => l.value);
    expect(registry).not.toContain('max');
    expect(registry).not.toContain('xhigh');
  });

  it('can widen a model too, so the heuristic never caps the registry', () => {
    const values = availableReasoningLevels('some-new-model', [
      'none', 'low', 'medium', 'high', 'xhigh', 'max',
    ]).map(l => l.value);
    expect(values).toContain('xhigh');
  });

  it('keeps the levels in ladder order however the set is ordered', () => {
    const values = availableReasoningLevels('m', ['high', 'none', 'medium']).map(l => l.value);
    expect(values).toEqual(['none', 'medium', 'high']);
  });

  it('falls back to the heuristic rather than offering nothing', () => {
    // An empty or unrecognisable set would render an empty dropdown, leaving
    // the user no way to pick at all.
    expect(availableReasoningLevels('gpt-5.4', []).map(l => l.value))
      .toEqual(['none', 'low', 'medium', 'high', 'xhigh']);
    expect(availableReasoningLevels('gpt-5.4', ['nonsense']).map(l => l.value))
      .toEqual(['none', 'low', 'medium', 'high', 'xhigh']);
  });
});
