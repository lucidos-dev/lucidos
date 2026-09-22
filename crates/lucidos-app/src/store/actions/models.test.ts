import { describe, it, expect, afterEach } from 'vitest';
import { chatModels, configuredProviders } from '../store';
import {
  chatModelOptions,
  clampEffortFor,
  isProviderConfigured,
  modelReasoningEfforts,
  parseContextWindow,
  lucidosModelChoices,
} from './models';
import { formatContextWindow } from '../../utils/formatTokens';
import { MODELS } from '../models';
import { displayModelName } from '../thread-events/exchange';
import type { ModelInfo } from '../../api/types';

function model(
  id: string,
  label: string,
  enabled = true,
  provider = 'anthropic',
  reasoning_efforts?: string[],
): ModelInfo {
  return {
    id,
    label,
    provider,
    sort_order: 0,
    source: 'user',
    enabled,
    context_window: null,
    created_at: '2026-01-01T00:00:00Z',
    ...(reasoning_efforts ? { reasoning_efforts } : {}),
  };
}

/** What the engine serves for a `provider = local` row. */
const LOCAL_TIERS = ['none', 'low', 'medium', 'high'];

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

  // The registry filters on `enabled`; the fallback cannot, so it must already
  // BE the enabled set. A retired model left here is offered for the moment
  // before `/models` lands, then disappears under the user's cursor.
  it('offers no model a disable migration has retired', () => {
    const retired = [
      'claude-opus-4-8@default',
      'claude-opus-4-8@default[1m]',
      'claude-opus-4-7',
      'claude-opus-4-7[1m]',
      'claude-sonnet-4-6',
      'claude-sonnet-4-6[1m]',
      'claude-opus-4-6',
      'claude-opus-4-5@20251101',
      'gpt-5.5',
      'gpt-5.4',
      'gpt-5.3-codex',
      'gpt-5.2-codex',
    ];
    expect(MODELS.filter((m) => retired.includes(m.value))).toEqual([]);
  });

  // Mirrors `DEFAULT_CHAT_MODEL` in core/preferences.rs. A fresh install with no
  // saved preference resolves to it, so the picker has to be able to show it.
  it('offers the default chat model', () => {
    expect(MODELS.some((m) => m.value === 'claude-opus-5@default')).toBe(true);
  });

  // The newest OpenAI builtin. A seed the fallback list misses is invisible
  // until `/models` lands, which is exactly when a fresh page is being read.
  it('offers GPT-6 Astra', () => {
    expect(MODELS).toContainEqual({ value: 'gpt-6-astra', label: 'GPT-6 Astra' });
  });

  // The newest Anthropic builtin, and the top of the list: its seed carries a
  // negative sort_order so it outranks Fable 5.
  it('offers Fable 5.1 above Fable 5', () => {
    expect(MODELS).toContainEqual({ value: 'claude-fable-5-1', label: 'Fable 5.1' });
    expect(MODELS).toContainEqual({ value: 'claude-fable-5-1[1m]', label: 'Fable 5.1 (1M)' });
    expect(MODELS.findIndex((m) => m.value === 'claude-fable-5-1')).toBeLessThan(
      MODELS.findIndex((m) => m.value === 'claude-fable-5'),
    );
  });

  // Seeded at sort_order 2/3, between Fable 5 and Opus 5, so the newest Opus
  // surfaces first while Opus 5 stays enabled below it.
  it('offers Opus 5.5 above Opus 5', () => {
    expect(MODELS).toContainEqual({ value: 'claude-opus-5-5', label: 'Opus 5.5' });
    expect(MODELS).toContainEqual({ value: 'claude-opus-5-5[1m]', label: 'Opus 5.5 (1M)' });
    expect(MODELS.findIndex((m) => m.value === 'claude-opus-5-5')).toBeLessThan(
      MODELS.findIndex((m) => m.value === 'claude-opus-5@default'),
    );
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

// The picker's half of the fix. `reasoningLevelsFor` / `clampEffortFor` are the
// ONLY way a Lucidos Agent surface should ask "what does this model support?",
// so the engine's answer is used wherever it exists and the id-shape heuristic
// is reached only when there is none.
describe('modelReasoningEfforts', () => {
  it('returns the registry answer for a loaded model', () => {
    chatModels.value = {
      status: 'loaded',
      data: [model('muse-glimmer:30b-mlx', 'Muse', true, 'local', LOCAL_TIERS)],
    };
    expect(modelReasoningEfforts('muse-glimmer:30b-mlx')).toEqual(LOCAL_TIERS);
  });

  it('cannot answer before the registry loads, or after it fails', () => {
    chatModels.value = { status: 'not-loaded' };
    expect(modelReasoningEfforts('muse-glimmer:30b-mlx')).toBeUndefined();
    chatModels.value = { status: 'loading' };
    expect(modelReasoningEfforts('muse-glimmer:30b-mlx')).toBeUndefined();
    chatModels.value = { status: 'failed', error: 'nope' };
    expect(modelReasoningEfforts('muse-glimmer:30b-mlx')).toBeUndefined();
  });

  it('cannot answer for an id with no row, or an engine predating the field', () => {
    chatModels.value = {
      status: 'loaded',
      data: [model('a', 'A', true, 'local', LOCAL_TIERS), model('older', 'Older')],
    };
    // A saved chat_model naming a model the user has since deleted.
    expect(modelReasoningEfforts('deleted-model')).toBeUndefined();
    // A row served without the field.
    expect(modelReasoningEfforts('older')).toBeUndefined();
  });
});

describe('lucidosModelChoices / clampEffortFor', () => {
  it('offers only what the engine says the model supports', () => {
    chatModels.value = {
      status: 'loaded',
      data: [model('muse-glimmer:30b-mlx', 'Muse', true, 'local', LOCAL_TIERS)],
    };
    const row = lucidosModelChoices().find((c) => c.value === 'muse-glimmer:30b-mlx');
    expect(row?.reasoningEfforts).toEqual(LOCAL_TIERS);
  });

  // The reported bug, at the layer the user touches: switching to this model
  // with the account effort at xhigh must land on a tier its server accepts.
  it('snaps xhigh onto the closest tier a local model supports', () => {
    chatModels.value = {
      status: 'loaded',
      data: [model('muse-glimmer:30b-mlx', 'Muse', true, 'local', LOCAL_TIERS)],
    };
    expect(clampEffortFor('xhigh', 'muse-glimmer:30b-mlx')).toBe('high');
    expect(clampEffortFor('max', 'muse-glimmer:30b-mlx')).toBe('high');
  });

  it('falls back to the id-shape heuristic when the registry cannot answer', () => {
    chatModels.value = { status: 'not-loaded' };
    // Pre-load, a GPT-5.6 id still gets its full set so the picker is usable.
    const row = lucidosModelChoices('gpt-5.6-sol').find((c) => c.value === 'gpt-5.6-sol');
    expect(row?.reasoningEfforts).toContain('max');
    // Astra too, which the heuristic missed while it keyed on `gpt-5.6` alone.
    const astra = lucidosModelChoices('gpt-6-astra').find((c) => c.value === 'gpt-6-astra');
    expect(astra?.reasoningEfforts).toContain('max');
    expect(clampEffortFor('max', 'gpt-6-astra')).toBe('max');
    expect(clampEffortFor('max', 'gpt-5.4')).toBe('xhigh');
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

  it('abbreviates declared windows to M and k', () => {
    // A million-token row reads as the marker its id carries, not as 1049k.
    expect(formatContextWindow(1048576)).toBe('context window: 1M');
    expect(formatContextWindow(1000000)).toBe('context window: 1M');
    expect(formatContextWindow(1050000)).toBe('context window: 1.1M');
    expect(formatContextWindow(200000)).toBe('context window: 200k');
    expect(formatContextWindow(512)).toBe('context window: 512');
  });

  it('never prints 1000k, whichever side of a million the value falls', () => {
    // Rounds up to 1000k, so a threshold on the raw value would print it.
    expect(formatContextWindow(999_999)).toBe('context window: 1M');
    expect(formatContextWindow(999_400)).toBe('context window: 999k');
  });
});

describe('displayModelName', () => {
  it('resolves Fable 5 from the static fallback labels', () => {
    chatModels.value = { status: 'not-loaded' };
    expect(displayModelName('claude-fable-5')).toBe('Fable 5');
  });

  // `claude-fable-5` is a prefix of `claude-fable-5-1`, so a lookup that was
  // not an exact match would label a 5.1 exchange as Fable 5.
  it('tells the two Fable generations apart', () => {
    chatModels.value = { status: 'not-loaded' };
    expect(displayModelName('claude-fable-5-1')).toBe('Fable 5.1');
    expect(displayModelName('claude-fable-5-1[1m]')).toBe('Fable 5.1 (1M)');
  });

  // `claude-opus-5` is a prefix of `claude-opus-5-5`, the same trap the Fable
  // pair sets. It also covers the bare id Claude Code echoes for the
  // `@default` row, which the registry knows only under the pinned spelling.
  it('tells the two Opus 5 generations apart', () => {
    chatModels.value = { status: 'not-loaded' };
    expect(displayModelName('claude-opus-5-5')).toBe('Opus 5.5');
    expect(displayModelName('claude-opus-5-5[1m]')).toBe('Opus 5.5 (1M)');
    expect(displayModelName('claude-opus-5')).toBe('Opus 5');
    expect(displayModelName('claude-opus-5[1m]')).toBe('Opus 5 (1M)');
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
