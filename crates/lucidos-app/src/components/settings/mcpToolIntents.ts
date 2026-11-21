import type { McpServersResponse } from '../../api/client';
import { patchToolDisabled } from './mcpCost';

/** A tool switch the engine has not yet confirmed, a *pending intent* (ADR
 *  0437). `ackedAt` is the list fetch epoch current when the engine answered,
 *  or null while the switch is still out. A list fetched after that epoch
 *  reflects the switch, so it ends the intent. */
export interface ToolIntent {
  serverId: string;
  wireName: string;
  disabled: boolean;
  ackedAt: number | null;
}

/** Every pending switch, keyed by `toolIntentKey`. */
export type ToolIntents = ReadonlyMap<string, ToolIntent>;

export function toolIntentKey(serverId: string, wireName: string): string {
  return `${serverId}\u0000${wireName}`;
}

/** The served list with every pending switch laid over it. Figures stay the
 *  engine's: only the switches move. */
export function withToolIntents(data: McpServersResponse, intents: ToolIntents): McpServersResponse {
  let shown = data;
  for (const intent of intents.values()) {
    shown = patchToolDisabled(shown, intent.serverId, intent.wireName, intent.disabled);
  }
  return shown;
}

/** The intents a list fetched at `servedEpoch` has not yet caught up with.
 *  Whatever that list says about an acknowledged switch is the engine's word
 *  now, even if another device has flipped it back since. */
export function intentsAfterServe(intents: ToolIntents, servedEpoch: number): ToolIntents {
  const kept = new Map<string, ToolIntent>();
  for (const [key, intent] of intents) {
    if (intent.ackedAt === null || servedEpoch <= intent.ackedAt) kept.set(key, intent);
  }
  return kept.size === intents.size ? intents : kept;
}
