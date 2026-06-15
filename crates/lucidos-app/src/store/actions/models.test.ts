import { describe, it, expect, afterEach } from 'vitest';
import { chatModels } from '../store';
import { chatModelOptions } from './models';
import { MODELS } from '../models';
import { displayModelName } from '../thread-events/exchange';
import type { ModelInfo } from '../../api/types';

function model(id: string, label: string, enabled = true): ModelInfo {
  return {
    id,
    label,
    provider: 'anthropic',
    sort_order: 0,
    source: 'user',
    enabled,
    created_at: '2026-01-01T00:00:00Z',
  };
}

afterEach(() => {
  chatModels.value = { status: 'not-loaded' };
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
