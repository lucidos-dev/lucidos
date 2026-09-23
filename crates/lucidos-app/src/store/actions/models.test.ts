import { describe, it, expect, afterEach } from 'vitest';
import { chatModels, configuredProviders } from '../store';
import {
  chatModelOptions,
  clampEffortFor,
  isProviderConfigured,
  modelReasoningEfforts,
  parseContextWindow,
  lucidosModelChoices,
  resolvedRoute,
  LUCIDOS_TIER_VOCABULARY,
  buildRoutes,
  routeDrafts,
} from './models';
import { modelRows } from '../modelSelection';
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
    routes: [{ provider, id, reasoning_efforts: reasoning_efforts ?? [] }],
    preferred_provider: null,
    sort_order: 0,
    source: 'user',
    enabled,
    created_at: '2026-01-01T00:00:00Z',
  };
}

/** A row served by two backends, which is what the Claude seeds ship. */
function dualRouted(
  id: string,
  label: string,
  providers: string[],
  preferred: string | null = null,
): ModelInfo {
  return {
    ...model(id, label),
    routes: providers.map((provider) => ({ provider, id, reasoning_efforts: [] })),
    preferred_provider: preferred,
  };
}

/** What the engine serves for a `provider = local` row. A third-party
 *  OpenAI-compatible server stops at `high`, since `xhigh` is OpenAI's own. */
const LOCAL_TIERS = ['none', 'low', 'medium', 'high'];

/** What an adaptive Claude route offers. */
const ALL_TIERS = ['none', 'low', 'medium', 'high', 'xhigh', 'max'];

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
      'claude-opus-4-8',
      'claude-opus-4-8[1m]',
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

  // THE REPORTED BUG. Every Claude Opus and Sonnet row was seeded on `vertex`,
  // and the filter asked about that one provider. A workspace whose only
  // credential is an Anthropic key therefore got the Fable rows and nothing
  // else from the Claude family.
  it('offers a dual-routed model when ANY of its backends is configured', () => {
    const opus = dualRouted('claude-opus-5-5', 'Opus 5.5', ['vertex', 'anthropic']);
    chatModels.value = { status: 'loaded', data: [opus] };

    configuredProviders.value = ['anthropic'];
    expect(chatModelOptions()).toEqual([{ value: 'claude-opus-5-5', label: 'Opus 5.5' }]);
    expect(resolvedRoute('claude-opus-5-5')?.provider).toBe('anthropic');

    // Vertex is listed first, so a workspace holding both keeps Vertex.
    configuredProviders.value = ['vertex', 'anthropic'];
    expect(resolvedRoute('claude-opus-5-5')?.provider).toBe('vertex');

    // Neither configured: the model leaves the picker, as it always did.
    configuredProviders.value = [];
    expect(chatModelOptions()).toEqual([]);
  });

  // Honoured or refused, never substituted. A model whose PICKED backend is
  // parked stays listed, because the provider step is the only place the user
  // can fix it. Hiding the model would be a dead end.
  it('keeps a model listed when its preferred backend is parked', () => {
    const opus = dualRouted(
      'claude-opus-5-5',
      'Opus 5.5',
      ['vertex', 'anthropic'],
      'anthropic',
    );
    chatModels.value = { status: 'loaded', data: [opus] };
    configuredProviders.value = ['vertex'];

    expect(chatModelOptions()).toEqual([{ value: 'claude-opus-5-5', label: 'Opus 5.5' }]);
    // And the picker names the PARKED backend, not the one that happens to be
    // configured, so what it shows is what the turn will do: refuse.
    expect(resolvedRoute('claude-opus-5-5')?.provider).toBe('anthropic');
  });

  // The refusal is fixable without leaving the picker: the provider step lists
  // every backend, marks the parked one, and shows it as the one in force.
  it('offers the parked backend in a provider step beside the configured one', () => {
    const opus = dualRouted(
      'claude-opus-5-5',
      'Opus 5.5',
      ['vertex', 'anthropic'],
      'anthropic',
    );
    chatModels.value = { status: 'loaded', data: [opus] };
    configuredProviders.value = ['vertex'];

    const [choice] = lucidosModelChoices();
    expect(choice.defaultProvider).toBe('anthropic');
    expect(choice.providers?.map((p) => [p.value, p.configured])).toEqual([
      ['vertex', true],
      ['anthropic', false],
    ]);
    const [row] = modelRows([choice], LUCIDOS_TIER_VOCABULARY);
    expect(row.provider).toBe('anthropic');
    expect(row.providers.map((p) => p.value)).toEqual(['vertex', 'anthropic']);
  });

  // One configured backend is no choice, so an Anthropic-only workspace picks
  // Opus in two steps, exactly as before routes.
  it('offers no provider step when only one backend is configured', () => {
    chatModels.value = {
      status: 'loaded',
      data: [dualRouted('claude-opus-5-5', 'Opus 5.5', ['vertex', 'anthropic'])],
    };
    configuredProviders.value = ['anthropic'];
    const [row] = modelRows(lucidosModelChoices(), LUCIDOS_TIER_VOCABULARY);
    expect(row.provider).toBe('anthropic');
    expect(row.providers).toEqual([]);
  });

  // Mirrors `DEFAULT_CHAT_MODEL` in core/preferences.rs. A fresh install with no
  // saved preference resolves to it, so the picker has to be able to show it.
  it('offers the default chat model', () => {
    expect(MODELS.some((m) => m.value === 'claude-opus-5')).toBe(true);
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
      MODELS.findIndex((m) => m.value === 'claude-opus-5'),
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

  it('cannot answer for an id with no row at all', () => {
    chatModels.value = {
      status: 'loaded',
      data: [model('a', 'A', true, 'local', LOCAL_TIERS), model('older', 'Older')],
    };
    // A saved chat_model naming a model the user has since deleted.
    expect(modelReasoningEfforts('deleted-model')).toBeUndefined();
    // A row always carries its route's set, so this is never `undefined`. An
    // EMPTY set falls through to the id-shape heuristic downstream, in
    // `availableReasoningLevels`, rather than rendering an empty dropdown.
    expect(modelReasoningEfforts('older')).toEqual([]);
  });

  // The set is per ROUTE, because the answer depends on the backend as much as
  // on the model. Resolution picks the route that will serve, so switching the
  // preferred provider switches which set the picker offers.
  it('answers for the route that would serve, not the row', () => {
    chatModels.value = {
      status: 'loaded',
      data: [
        {
          ...model('claude-opus-5-5', 'Opus 5.5'),
          routes: [
            { provider: 'vertex', id: 'claude-opus-5-5', reasoning_efforts: ALL_TIERS },
            {
              provider: 'openrouter',
              id: 'anthropic/claude-opus-5-5',
              reasoning_efforts: LOCAL_TIERS,
            },
          ],
          preferred_provider: 'openrouter',
        },
      ],
    };
    configuredProviders.value = ['vertex', 'openrouter'];
    expect(modelReasoningEfforts('claude-opus-5-5')).toEqual(LOCAL_TIERS);
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
  // pair sets.
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

describe('the route editor', () => {
  const opus = dualRouted('claude-opus-5-5', 'Opus 5.5', ['vertex', 'anthropic']);

  it('reads a wire id equal to the model id as blank', () => {
    expect(routeDrafts(opus)).toEqual([
      { provider: 'vertex', id: '', contextWindow: '' },
      { provider: 'anthropic', id: '', contextWindow: '' },
    ]);
  });

  it('sends only what differs from the defaults', () => {
    expect(buildRoutes('claude-opus-5-5', [
      { provider: 'vertex', id: '', contextWindow: '' },
      { provider: 'openrouter', id: 'anthropic/claude-opus-5-5', contextWindow: '200000' },
    ])).toEqual({
      ok: true,
      routes: [
        { provider: 'vertex' },
        { provider: 'openrouter', id: 'anthropic/claude-opus-5-5', context_window: 200000 },
      ],
    });
  });

  it('names the field that is wrong', () => {
    expect(buildRoutes('m', [])).toEqual({ ok: false, error: 'A model needs at least one route' });
    const doubled = buildRoutes('m', [
      { provider: 'vertex', id: '', contextWindow: '' },
      { provider: 'vertex', id: '', contextWindow: '' },
    ]);
    expect(doubled).toEqual({ ok: false, error: 'Vertex appears twice' });
    const badWindow = buildRoutes('m', [{ provider: 'xai', id: '', contextWindow: 'big' }]);
    expect(badWindow.ok).toBe(false);
  });
});
