/** Default chat model when no preference is set.
 *  Mirrored on the backend in `crates/lucidos-engine/src/core/preferences.rs`. */
export const DEFAULT_CHAT_MODEL = 'claude-opus-5@default';

/** Fallback chat model options shown before the DB-backed registry (`/models`)
 *  loads, and used by tests + label lookups. The live picker reads the loaded
 *  registry via `chatModelOptions()` (store/actions/models.ts); this list is the
 *  startup fallback so the picker never renders empty. Keep roughly in sync with
 *  the migration seed in `20260610152555_create_models_registry.sql`. */
export const MODELS = [
  { value: 'claude-fable-5', label: 'Fable 5' },
  { value: 'claude-fable-5[1m]', label: 'Fable 5 (1M)' },
  { value: 'claude-opus-5@default', label: 'Opus 5' },
  { value: 'claude-opus-5@default[1m]', label: 'Opus 5 (1M)' },
  { value: 'claude-sonnet-5', label: 'Sonnet 5' },
  { value: 'claude-sonnet-5[1m]', label: 'Sonnet 5 (1M)' },
  { value: 'claude-opus-4-8@default', label: 'Opus 4.8' },
  { value: 'claude-opus-4-8@default[1m]', label: 'Opus 4.8 (1M)' },
  { value: 'claude-opus-4-7', label: 'Opus 4.7' },
  { value: 'claude-opus-4-7[1m]', label: 'Opus 4.7 (1M)' },
  { value: 'claude-sonnet-4-6', label: 'Sonnet 4.6' },
  { value: 'claude-sonnet-4-6[1m]', label: 'Sonnet 4.6 (1M)' },
  { value: 'gemini-3.1-pro-preview', label: 'Gemini 3.1 Pro' },
  { value: 'gemini-3.5-flash', label: 'Gemini 3.5 Flash' },
  { value: 'gemini-3-flash-preview', label: 'Gemini 3 Flash' },
  { value: 'gpt-5.6-sol', label: 'GPT-5.6 Sol' },
  { value: 'gpt-5.6-terra', label: 'GPT-5.6 Terra' },
  { value: 'gpt-5.6-luna', label: 'GPT-5.6 Luna' },
  { value: 'gpt-5.5-pro', label: 'GPT-5.5 Pro' },
  { value: 'gpt-5.5', label: 'GPT-5.5' },
  { value: 'gpt-5.4', label: 'GPT-5.4' },
  { value: 'gpt-5.3-codex', label: 'GPT-5.3 Codex' },
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

/** Levels in ascending order — used for clamping when a level is unavailable. */
const REASONING_ORDER = REASONING_LEVELS.map(l => l.value);

/** Filter REASONING_LEVELS to those the given model actually supports.
 *
 *  `supported` is the model's `reasoning_efforts` from the `/models` registry:
 *  the engine's own answer (`llm::reasoning::supported_efforts`), and the same
 *  set `RoutingProvider` clamps the request onto. **Pass it whenever you have
 *  it**, by going through `reasoningLevelsFor` / `clampEffortFor` in
 *  `store/actions/models.ts`, which look it up. Deriving the answer here
 *  independently is what produced the bug this argument exists to close: the
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

/** Snap an effort to the nearest level the given model supports.
 *  Ties break toward the higher level — switching providers shouldn't quietly
 *  reduce the user's effort intent (e.g. xhigh on a Claude budget model snaps to
 *  max, not high). Mirrors `llm::reasoning::clamp_effort`, which enforces the
 *  same nearest-with-ties-upward rule on the request itself; keep the two in
 *  step. `supported` is the registry's answer, see `availableReasoningLevels`.
 *
 *  They diverge in ONE case, deliberately: a value that is not a level at all.
 *  Rust DROPS it and lets the provider default apply, because inventing a tier
 *  would make a typo silently buy the most expensive reasoning the model has.
 *  Here the caller is a dropdown that has to show something selected, so an
 *  unrecognised value takes the top offered level. Safe because it is a display
 *  choice, not a billing one: whatever this returns is re-checked at the wire,
 *  and the two paths that could carry junk are already filtered before they
 *  reach here (`currentChatReasoningEffort` validates against REASONING_LEVELS,
 *  and a picked value comes from the offered list). */
export function clampReasoningEffort(
  effort: string,
  model: string,
  supported?: readonly string[],
): string {
  const available = availableReasoningLevels(model, supported);
  if (available.some(l => l.value === effort)) return effort;
  const target = REASONING_ORDER.indexOf(effort);
  if (target === -1) return available[available.length - 1].value;
  return available
    .map(l => ({ value: l.value, dist: Math.abs(REASONING_ORDER.indexOf(l.value) - target) }))
    .reduce((best, cur) => (cur.dist <= best.dist ? cur : best))
    .value;
}
