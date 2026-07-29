import { describe, it, expect, afterEach } from 'vitest';
import { chatModels, configuredProviders } from '../store';
import { chatModelOptions, isProviderConfigured, parseContextWindow } from './models';
import { formatContextWindow } from '../../utils/formatTokens';
import { MODELS } from '../models';
import { displayModelName } from '../thread-events/exchange';
import type { ModelInfo } from '../../api/types';

function model(id: string, label: string, enabled = true, provider = 'anthropic'): ModelInfo {
  return {
    id,
    label,
    provider,
    sort_order: 0,
    source: 'user',
    enabled,
    context_window: null,
    created_at: '2026-01-01T00:00:00Z',
  };
}

afterEach(() => {
  chatModels.value = { status: 'not-loaded' };
  configuredProviders.value = null;
});

describe('chatModelOptions', () => {
  it('falls back to the static MODELS list before the registry loads', () => {
    chatModels.value = { status: 'not-loaded' };
    expect(chatModelOptions()).toBe(MODELS);
    // The fallback includes Fable 5 so the picker is never empty pre-load.
    expect(MODELS.some((m) => m.value === 'claude-fable-5')).toBe(true);
  });

  it('returns only enabled models, mapped to {value,label}, when loaded', () => {
    chatModels.value = {
      status: 'loaded',
      data: [model('a', 'A'), model('b', 'B', false), model('c', 'C')],
    };
    expect(chatModelOptions()).toEqual([
      { value: 'a', label: 'A' },
      { value: 'c', label: 'C' },
    ]);
  });

  it('filters to models whose provider is configured', () => {
    chatModels.value = {
      status: 'loaded',
      data: [
        model('gpt', 'GPT', true, 'openai'),
        model('claude-vertex', 'Claude (Vertex)', true, 'vertex'),
        model('glm', 'GLM', true, 'openrouter'),
      ],
    };
    configuredProviders.value = ['openai'];
    expect(chatModelOptions()).toEqual([{ value: 'gpt', label: 'GPT' }]);
  });

  it('does not filter when the configured set is null (mock / older engine)', () => {
    chatModels.value = {
      status: 'loaded',
      data: [model('gpt', 'GPT', true, 'openai'), model('v', 'V', true, 'vertex')],
    };
    configuredProviders.value = null;
    expect(chatModelOptions()).toEqual([
      { value: 'gpt', label: 'GPT' },
      { value: 'v', label: 'V' },
    ]);
  });

  it('hides everything when no provider is configured (empty set)', () => {
    chatModels.value = { status: 'loaded', data: [model('gpt', 'GPT', true, 'openai')] };
    configuredProviders.value = [];
    expect(chatModelOptions()).toEqual([]);
  });
});

describe('isProviderConfigured', () => {
  it('treats every provider as configured when the set is null', () => {
    configuredProviders.value = null;
    expect(isProviderConfigured('vertex')).toBe(true);
    expect(isProviderConfigured('anything')).toBe(true);
  });

  it('matches against the configured set when present', () => {
    configuredProviders.value = ['openai', 'vertex'];
    expect(isProviderConfigured('openai')).toBe(true);
    expect(isProviderConfigured('vertex')).toBe(true);
    expect(isProviderConfigured('anthropic')).toBe(false);
  });
});

describe('parseContextWindow', () => {
  it('treats blank as "infer from the id"', () => {
    expect(parseContextWindow('')).toEqual({ ok: true, value: undefined });
    expect(parseContextWindow('   ')).toEqual({ ok: true, value: undefined });
  });

  it('accepts a positive whole number of tokens', () => {
    expect(parseContextWindow('1048576')).toEqual({ ok: true, value: 1048576 });
    expect(parseContextWindow(' 200000 ')).toEqual({ ok: true, value: 200000 });
  });

  it('rejects non-positive and non-integer values rather than dropping them', () => {
    // Silently dropping a bad value would leave the model on the 200k default
    // with no sign anything went wrong — the exact failure this field exists
    // to prevent.
    for (const bad of ['0', '-1', 'abc', '1.5', '1e', '']) {
      if (bad === '') continue;
      expect(parseContextWindow(bad).ok).toBe(false);
    }
  });
});

describe('formatContextWindow', () => {
  it('says "inferred" when the row declares nothing', () => {
    expect(formatContextWindow(null)).toBe('context window: inferred');
  });

  it('abbreviates declared windows to k', () => {
    expect(formatContextWindow(1048576)).toBe('context window: 1049k');
    expect(formatContextWindow(200000)).toBe('context window: 200k');
    expect(formatContextWindow(512)).toBe('context window: 512');
  });
});

describe('displayModelName', () => {
  it('resolves Fable 5 from the static fallback labels', () => {
    chatModels.value = { status: 'not-loaded' };
    expect(displayModelName('claude-fable-5')).toBe('Fable 5');
  });

  it('prefers the loaded registry label for a user-added model', () => {
    chatModels.value = { status: 'loaded', data: [model('my-model', 'My Custom Model')] };
    expect(displayModelName('my-model')).toBe('My Custom Model');
  });

  it('falls back to the raw id for an unknown model', () => {
    chatModels.value = { status: 'not-loaded' };
    expect(displayModelName('totally-unknown')).toBe('totally-unknown');
  });
});
