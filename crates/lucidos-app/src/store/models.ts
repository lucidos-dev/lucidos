/** Default chat model when no preference is set.
 *  Mirrored on the backend in `crates/lucidos-engine/src/core/preferences.rs`. */
export const DEFAULT_CHAT_MODEL = 'claude-opus-5@default';

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
  { value: 'claude-fable-5', label: 'Fable 5' },
  { value: 'claude-fable-5[1m]', label: 'Fable 5 (1M)' },
  { value: 'claude-opus-5@default', label: 'Opus 5' },
  { value: 'claude-opus-5@default[1m]', label: 'Opus 5 (1M)' },
  { value: 'claude-sonnet-5', label: 'Sonnet 5' },
  { value: 'claude-sonnet-5[1m]', label: 'Sonnet 5 (1M)' },
  { value: 'gemini-3.1-pro-preview', label: 'Gemini 3.1 Pro' },
  { value: 'gemini-3.5-flash', label: 'Gemini 3.5 Flash' },
  { value: 'gemini-3-flash-preview', label: 'Gemini 3 Flash' },
  { value: 'gpt-5.6-sol', label: 'GPT-5.6 Sol' },
  { value: 'gpt-5.6-terra', label: 'GPT-5.6 Terra' },
  { value: 'gpt-5.6-luna', label: 'GPT-5.6 Luna' },
  { value: 'gpt-5.5-pro', label: 'GPT-5.5 Pro' },
];

/** Reasoning effort levels for Claude Code thinking budget. */
export const REASONING_LEVELS = [
  { value: 'none', label: 'Off' },
  { value: 'low', label: 'Low' },
  { value: 'medium', label: 'Med' },
  { value: 'high', label: 'High' },
  { value: 'xhigh', label: 'X-High' },
  { value: 'max', label: 'Max' },
];

/** Filter REASONING_LEVELS to those the given model actually supports.
 *
 *  `supported` is the model's `reasoning_efforts` from the `/models` registry:
 *  the engine's own answer (`llm::reasoning::supported_efforts`), and the same
 *  set `RoutingProvider` clamps the request onto. **Pass it whenever you have
 *  it**, by going through `lucidosTiers` in `store/modelSelection.ts`, paired
 *  with the registry lookup in `store/actions/models.ts`. Deriving the answer
 *  here independently is what produced the bug this argument closes: the
 *  heuristic below matches on the model id's SHAPE, which says nothing about
 *  which server serves the model. A local `muse-glimmer:30b-mlx` matched no
 *  branch, fell through to the Gemini-shaped default, and was offered `max`;
 *  the engine then sent something else and its server 400'd.
 *
 *  The heuristic is the fallback for the two cases with no registry answer: the
 *  picker rendering before `/models` lands, and a saved `chat_model` naming an
 *  id with no row. It is deliberately unchanged from when it was the only rule:
 *  - GPT-5.6 (Sol / Terra / Luna): full set — the family adds a distinct `max`
 *    reasoning tier (Sol's headline "Max reasoning effort").
 *  - Other OpenAI: drops `max` (their top tier is `xhigh`, so `max` would be a duplicate).
 *  - Fable 5 / Opus 4.7+ (incl. Opus 5) / Sonnet 5: full set (the adaptive Anthropic family that
 *    natively supports `xhigh`). Sonnet 5 is the first Sonnet-tier model with a distinct `xhigh`;
 *    Sonnet 4.6 and older stay on the filtered set below.
 *  - Other Claude / Gemini: drops `xhigh` (not a distinct tier on those backends). */
export function availableReasoningLevels(
  model: string,
  supported?: readonly string[],
): typeof REASONING_LEVELS {
  if (supported) {
    const offered = REASONING_LEVELS.filter(l => supported.includes(l.value));
    // An empty result would render an empty dropdown, so a registry row that
    // declares nothing we recognise falls through to the heuristic rather than
    // leaving the user no way to pick.
    if (offered.length > 0) return offered;
  }
  if (model.startsWith('gpt-')) {
    if (model.startsWith('gpt-5.6')) return REASONING_LEVELS;
    return REASONING_LEVELS.filter(l => l.value !== 'max');
  }
  if (
    model.startsWith('claude-opus-4-7') ||
    model.startsWith('claude-opus-4-8') ||
    model.startsWith('claude-opus-5') ||
    model.startsWith('claude-sonnet-5') ||
    model.startsWith('claude-fable-5')
  ) {
    return REASONING_LEVELS;
  }
  return REASONING_LEVELS.filter(l => l.value !== 'xhigh');
}
