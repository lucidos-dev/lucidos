
import {
  findCommandPermissionResolution,
  findMcpPermissionResolution,
  findPermissionResolution,
  findQuestionAnswer,
  questionDividerResolution,
  isChangeLifecycleEvent,
  legacyOrigin,
  starterEngineReason,
  type AnsweredQuestion,
  type ResolvedCommandPermission,
  type ResolvedMcpPermission,
  type ResolvedPermission,
  type Exchange,
  type EngineReason,
  type MessageOrigin,
} from '../../store/thread-events';

const CONTINUATION_ENGINE_REASON: EngineReason = { kind: 'continuation_started' };

export function resolveOrigin(exchange: Exchange): MessageOrigin | undefined {
  const userEvent = exchange.userEvent;
  if (userEvent.type === 'MessageReceived') return legacyOrigin(userEvent);
  if (isChangeLifecycleEvent(userEvent)) return userEvent.actor;
  const settlement = dividerSettlement(exchange);
  if (settlement !== null) return settlement?.actor;
  if ('origin' in userEvent && userEvent.origin) {
    return userEvent.origin as MessageOrigin;
  }
  if ('actor' in userEvent && userEvent.actor) {
    return userEvent.actor as MessageOrigin;
  }
  const implied = starterEngineReason(userEvent)
    ?? (userEvent.type === 'ContinuationStarted' ? CONTINUATION_ENGINE_REASON : undefined);
  if (implied) return { kind: 'engine', reason: implied };
  return undefined;
}

export type DividerSettlement =
  | AnsweredQuestion
  | ResolvedPermission
  | ResolvedCommandPermission
  | ResolvedMcpPermission;

export function dividerSettlement(exchange: Exchange): DividerSettlement | undefined | null {
  const userEvent = exchange.userEvent;
  switch (userEvent.type) {
    case 'UserQuestionAsked':
      return questionDividerResolution(exchange) ? undefined : findQuestionAnswer(exchange, userEvent.tool_use_id);
    case 'CodingAgentPermissionRequest':
      return findPermissionResolution(exchange, userEvent.request_id);
    case 'CommandPermissionRequested':
      return findCommandPermissionResolution(exchange, userEvent.request_id);
    case 'McpPermissionRequested':
      return findMcpPermissionResolution(exchange, userEvent.request_id);
    case 'McpConsentRequested':
      return undefined;
    default:
      return null;
  }
}

export function originPopoverHasContent(exchange: Exchange): boolean {
  return dividerSettlement(exchange) === null || resolveOrigin(exchange) !== undefined;
}
