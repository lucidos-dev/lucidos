/** The account's chat tier per model: the `chat_reasoning_efforts` preference,
 *  `model=tier` pairs separated by commas. A model not listed runs at its
 *  *default effort*.
 *
 *  Mirrors the engine's `parse_model_efforts`, except that a pair the engine
 *  would refuse is skipped rather than failing the whole value: the engine
 *  validates every write, so only a hand-edited row can hold one. */

import { EFFORT_LADDER } from './modelSelection';

/** Each listed model's tier. A model named twice keeps its last tier. */
export function parseChatEfforts(value: string | null | undefined): Map<string, string> {
  const efforts = new Map<string, string>();
  for (const item of (value ?? '').split(',')) {
    const at = item.indexOf('=');
    if (at < 0) continue;
    const model = item.slice(0, at).trim();
    const tier = item.slice(at + 1).trim();
    if (model !== '' && EFFORT_LADDER.includes(tier)) efforts.set(model, tier);
  }
  return efforts;
}

/** `value` with `model`'s tier set to `tier`, or removed for `null`, every
 *  other model kept. An empty result reads as unset. */
export function withChatEffort(
  value: string | null | undefined,
  model: string,
  tier: string | null,
): string {
  const efforts = parseChatEfforts(value);
  if (tier === null) efforts.delete(model);
  else efforts.set(model, tier);
  return [...efforts].map(([id, t]) => `${id}=${t}`).join(', ');
}
