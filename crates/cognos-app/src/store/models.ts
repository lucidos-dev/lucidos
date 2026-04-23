/** Default chat model when no preference is set.
 *  Mirrored on the backend in `crates/cognos-engine/src/core/preferences.rs`. */
export const DEFAULT_CHAT_MODEL = 'claude-opus-4-7';

/** CognOS chat model options — used for display labels and model pickers. */
export const MODELS = [
  { value: 'claude-opus-4-7', label: 'Opus 4.7' },
  { value: 'claude-opus-4-7[1m]', label: 'Opus 4.7 (1M)' },
  { value: 'claude-opus-4-6', label: 'Opus 4.6' },
  { value: 'claude-opus-4-6[1m]', label: 'Opus 4.6 (1M)' },
  { value: 'claude-sonnet-4-6', label: 'Sonnet 4.6' },
  { value: 'claude-sonnet-4-6[1m]', label: 'Sonnet 4.6 (1M)' },
  { value: 'claude-opus-4-5@20251101', label: 'Opus 4.5' },
  { value: 'gemini-3.1-pro-preview', label: 'Gemini 3.1 Pro' },
  { value: 'gemini-3-flash-preview', label: 'Gemini 3 Flash' },
  { value: 'gpt-5.4', label: 'GPT-5.4' },
  { value: 'gpt-5.3-codex', label: 'GPT-5.3 Codex' },
  { value: 'gpt-5.3-codex-spark', label: 'Codex Spark' },
  { value: 'gpt-5.2-codex', label: 'GPT-5.2 Codex' },
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
 *  - OpenAI: drops `max` (its top tier is `xhigh`, so `max` would be a duplicate).
 *  - Opus 4.7: full set (only adaptive Anthropic model that natively supports `xhigh`).
 *  - Other Claude / Gemini: drops `xhigh` (not a distinct tier on those backends). */
export function availableReasoningLevels(model: string): typeof REASONING_LEVELS {
  if (model.startsWith('gpt-')) {
    return REASONING_LEVELS.filter(l => l.value !== 'max');
  }
  if (model.startsWith('claude-opus-4-7')) {
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
