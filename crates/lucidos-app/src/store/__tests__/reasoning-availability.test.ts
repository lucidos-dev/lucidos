import { describe, it, expect } from 'vitest';
import { availableReasoningLevels, clampReasoningEffort } from '../models';

describe('availableReasoningLevels', () => {
  it('drops max for pre-5.6 OpenAI models (collapses with xhigh)', () => {
    const values = availableReasoningLevels('gpt-5.4').map(l => l.value);
    expect(values).toEqual(['none', 'low', 'medium', 'high', 'xhigh']);
  });

  it('exposes full set (incl max) for the GPT-5.6 family', () => {
    for (const model of ['gpt-5.6-sol', 'gpt-5.6-terra', 'gpt-5.6-luna']) {
      const values = availableReasoningLevels(model).map(l => l.value);
      expect(values).toEqual(['none', 'low', 'medium', 'high', 'xhigh', 'max']);
    }
  });

  it('exposes full set for Opus 4.7', () => {
    const values = availableReasoningLevels('claude-opus-4-7').map(l => l.value);
    expect(values).toEqual(['none', 'low', 'medium', 'high', 'xhigh', 'max']);
  });

  it('exposes full set for Opus 4.7 with 1M suffix', () => {
    const values = availableReasoningLevels('claude-opus-4-7[1m]').map(l => l.value);
    expect(values).toContain('xhigh');
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

describe('clampReasoningEffort', () => {
  it('keeps effort when supported', () => {
    expect(clampReasoningEffort('xhigh', 'claude-opus-4-7')).toBe('xhigh');
    expect(clampReasoningEffort('max', 'claude-opus-4-6')).toBe('max');
  });

  it('snaps xhigh to max on non-Opus-4.7 Claude', () => {
    expect(clampReasoningEffort('xhigh', 'claude-opus-4-6')).toBe('max');
  });

  it('snaps max to xhigh on pre-5.6 OpenAI', () => {
    expect(clampReasoningEffort('max', 'gpt-5.4')).toBe('xhigh');
  });

  it('keeps max on the GPT-5.6 family', () => {
    expect(clampReasoningEffort('max', 'gpt-5.6-sol')).toBe('max');
    expect(clampReasoningEffort('max', 'gpt-5.6-luna')).toBe('max');
  });

  it('snaps xhigh to max on Gemini (ties break toward higher effort)', () => {
    expect(clampReasoningEffort('xhigh', 'gemini-3.1-pro-preview')).toBe('max');
  });
});
