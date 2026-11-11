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
 *  MINUS whatever the disable migrations switched off, and every model a
 *  migration retired with `retire_model` (ADR 0418). An engine test fails
 *  while this list still offers a retired id. */
export const MODELS = [
  { value: 'claude-fable-5-2', label: 'Fable 5.2' },
  { value: 'gemini-4-carbon', label: 'Gemini 4 Carbon' },
  { value: 'gemini-4-argon', label: 'Gemini 4 Argon' },
  { value: 'claude-fable-5-1', label: 'Fable 5.1' },
  { value: 'claude-opus-5-5', label: 'Opus 5.5' },
  { value: 'claude-sonnet-5-5', label: 'Sonnet 5.5' },
  { value: 'claude-haiku-5-5', label: 'Haiku 5.5' },
  { value: 'gemini-3.1-pro-preview', label: 'Gemini 3.1 Pro' },
  { value: 'gemini-3.8-flash', label: 'Gemini 3.8 Flash' },
  { value: 'gemini-3.1-flash-lite', label: 'Gemini 3.1 Flash-Lite' },
  { value: 'gpt-6-astra', label: 'GPT-6 Astra' },
  { value: 'gpt-6.1-sol', label: 'GPT-6.1 Sol' },
  { value: 'gpt-6-luna', label: 'GPT-6 Luna' },
];

export { PROVIDERS, providerLabel } from './modelProviders';

export { REASONING_LEVELS, availableReasoningLevels } from './reasoningLevels';
