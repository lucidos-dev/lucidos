
import { EFFORT_LADDER } from './modelSelection';

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
