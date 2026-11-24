import type { McpServersResponse } from '../../api/client';
import { patchToolDisabled } from './mcpCost';

export interface ToolIntent {
  serverId: string;
  wireName: string;
  disabled: boolean;
  ackedAt: number | null;
}

export type ToolIntents = ReadonlyMap<string, ToolIntent>;

export function toolIntentKey(serverId: string, wireName: string): string {
  return `${serverId}\u0000${wireName}`;
}

export function withToolIntents(data: McpServersResponse, intents: ToolIntents): McpServersResponse {
  let shown = data;
  for (const intent of intents.values()) {
    shown = patchToolDisabled(shown, intent.serverId, intent.wireName, intent.disabled);
  }
  return shown;
}

export function intentsAfterServe(intents: ToolIntents, servedEpoch: number): ToolIntents {
  const kept = new Map<string, ToolIntent>();
  for (const [key, intent] of intents) {
    if (intent.ackedAt === null || servedEpoch <= intent.ackedAt) kept.set(key, intent);
  }
  return kept.size === intents.size ? intents : kept;
}
