import { PREFERENCE_CATALOG } from '@lucidos/preference-catalog';

/** Default chat model when no preference is set. */
export const DEFAULT_CHAT_MODEL: string = PREFERENCE_CATALOG.chat_model.fallback;

/** Fallback chat model options shown before the DB-backed registry (`/models`)
 *  loads, and used by tests + label lookups. The live picker reads the loaded
 *  registry via `chatModelOptions()` (store/actions/models.ts); this list is the
 *  startup fallback so the picker never renders empty.
 *
 *  It must hold the ENABLED builtins, not every seeded one. The registry filters
 *  on `enabled`, so a retired model listed here is offered until `/models`
 *  lands, then vanishes. Keep it in step with the seeds
 *  (`20260610152555_create_models_registry.sql` and the later per-family ones)
 *  MINUS whatever the disable migrations switched off, most recently
 *  `20260823080956_disable_prior_generation_builtin_models.sql`. */
export const MODELS = [
  { value: 'claude-fable-5-1', label: 'Fable 5.1' },
  { value: 'claude-fable-5-1[1m]', label: 'Fable 5.1 (1M)' },
  { value: 'claude-fable-5', label: 'Fable 5' },
  { value: 'claude-fable-5[1m]', label: 'Fable 5 (1M)' },
  { value: 'claude-opus-5-5', label: 'Opus 5.5' },
  { value: 'claude-opus-5-5[1m]', label: 'Opus 5.5 (1M)' },
  { value: 'claude-sonnet-5-5', label: 'Sonnet 5.5' },
  { value: 'claude-sonnet-5-5[1m]', label: 'Sonnet 5.5 (1M)' },
  { value: 'claude-opus-5', label: 'Opus 5' },
  { value: 'claude-opus-5[1m]', label: 'Opus 5 (1M)' },
  { value: 'claude-sonnet-5', label: 'Sonnet 5' },
  { value: 'claude-sonnet-5[1m]', label: 'Sonnet 5 (1M)' },
  { value: 'claude-haiku-5-5', label: 'Haiku 5.5' },
  { value: 'gemini-3.1-pro-preview', label: 'Gemini 3.1 Pro' },
  { value: 'gemini-3.5-flash', label: 'Gemini 3.5 Flash' },
  { value: 'gemini-3-flash-preview', label: 'Gemini 3 Flash' },
  { value: 'gpt-6-astra', label: 'GPT-6 Astra' },
  { value: 'gpt-5.6-sol', label: 'GPT-5.6 Sol' },
  { value: 'gpt-5.6-terra', label: 'GPT-5.6 Terra' },
  { value: 'gpt-5.6-luna', label: 'GPT-5.6 Luna' },
  { value: 'gpt-5.5-pro', label: 'GPT-5.5 Pro' },
];

/** Every backend a model route can name, with how the UI names it. Mirrors
 *  `llm::model_registry::ProviderKind`, whose `as_str` gives the values. */
export const PROVIDERS = [
  { value: 'anthropic', label: 'Anthropic (direct)' },
  { value: 'vertex', label: 'Vertex' },
  { value: 'openai', label: 'OpenAI' },
  { value: 'openrouter', label: 'OpenRouter' },
  { value: 'xai', label: 'xAI' },
  { value: 'opencode-free', label: 'OpenCode Free (keyless)' },
  { value: 'local', label: 'Local (OpenAI-compatible)' },
];

/** How the UI names a provider, falling back to the raw value. */
export function providerLabel(provider: string): string {
  return PROVIDERS.find((p) => p.value === provider)?.label ?? provider;
}

/** The unified effort ladder, ascending, in the Lucidos Agent's vocabulary. */
export const REASONING_LEVELS = [
  { value: 'none', label: 'Off' },
  { value: 'low', label: 'Low' },
  { value: 'medium', label: 'Med' },
  { value: 'high', label: 'High' },
  { value: 'xhigh', label: 'X-High' },
  { value: 'max', label: 'Max' },
];

/** The REASONING_LEVELS a model supports, by the engine's own answer.
 *
 *  `supported` is the model's `reasoning_efforts` from the `/models` registry
 *  (`llm::reasoning::supported_efforts`), the same set `RoutingProvider`
 *  clamps a request onto. Get it through `lucidosTiers` in
 *  `store/modelSelection.ts`. With no answer, every level is offered, and the
 *  engine snaps the request onto what the backend accepts. That covers a
 *  registry still loading and an id with no row. The client keeps no copy of the per-family
 *  rules, so it cannot offer a tier the engine has stopped honouring
 *  (ADR 0368). */
export function availableReasoningLevels(
  supported?: readonly string[],
): typeof REASONING_LEVELS {
  const offered = supported ? REASONING_LEVELS.filter(l => supported.includes(l.value)) : [];
  // An empty result would render an empty dropdown, so a registry row that
  // declares nothing we recognise offers the whole ladder instead.
  return offered.length > 0 ? offered : REASONING_LEVELS;
}
