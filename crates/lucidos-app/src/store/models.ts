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
 *  - GPT-5.6 (Sol / Terra / Luna): full set — the family adds a distinct `max`
 *    reasoning tier (Sol's headline "Max reasoning effort").
 *  - Other OpenAI: drops `max` (their top tier is `xhigh`, so `max` would be a duplicate).
 *  - Fable 5 / Opus 4.7+ (incl. Opus 5) / Sonnet 5: full set (the adaptive Anthropic family that
 *    natively supports `xhigh`). Sonnet 5 is the first Sonnet-tier model with a distinct `xhigh`;
 *    Sonnet 4.6 and older stay on the filtered set below.
 *  - Other Claude / Gemini: drops `xhigh` (not a distinct tier on those backends). */
export function availableReasoningLevels(model: string): typeof REASONING_LEVELS {
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
 *  reduce the user's effort intent (e.g. xhigh on Gemini snaps to max, not high). */
export function clampReasoningEffort(effort: string, model: string): string {
  const available = availableReasoningLevels(model);
  if (available.some(l => l.value === effort)) return effort;
  const target = REASONING_ORDER.indexOf(effort);
  if (target === -1) return available[available.length - 1].value;
  return available
    .map(l => ({ value: l.value, dist: Math.abs(REASONING_ORDER.indexOf(l.value) - target) }))
    .reduce((best, cur) => (cur.dist <= best.dist ? cur : best))
    .value;
}
