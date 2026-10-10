/** Who started a turn, read from its exchange. The transcript asks this on
 *  every turn, so it lives apart from the lazy `MessageRoutePanel.tsx`. A
 *  static import of the panel would pull it into the shell chunk. */

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

/** A bare `ContinuationStarted` is engine-raised by *current* design on the
 *  chat and trigger channels: `emit_resume_anchor` (`chat/rerun.rs`) writes
 *  `origin: None` on every resume, carrying its attribution on `EventMeta.actor`
 *  instead. So it lands here for the *actor-less* case only: a user-clicked
 *  Continue stamps the clicking device on `actor`, and `resolveOrigin` prefers
 *  that. Its explainer comes from the event's own `reason` field where one was
 *  recorded, not from this coarser `continuation_started`.
 *
 *  Every other engine-only starter resolves through `starterEngineReason`. */
const CONTINUATION_ENGINE_REASON: EngineReason = { kind: 'continuation_started' };

/** Takes the full Exchange for the divider starters (UserQuestionAsked,
 *  CodingAgentPermissionRequest). Their device actor is on the matching
 *  resolution event, found in the exchange's steps. */
export function resolveOrigin(exchange: Exchange): MessageOrigin | undefined {
  const userEvent = exchange.userEvent;
  if (userEvent.type === 'MessageReceived') return legacyOrigin(userEvent);
  if (isChangeLifecycleEvent(userEvent)) return userEvent.actor;
  // A question or permission card: the chip names the agent that asked, so the
  // Origin is whoever answered it.
  const settlement = dividerSettlement(exchange);
  if (settlement !== null) return settlement?.actor;
  // Engine-emitted events (ContinuationStarted, CodingAgentPromptSent, TriggerStarted, ChangeProposed)
  // carry origin directly — surface it so the popover can render the Engine variant.
  if ('origin' in userEvent && userEvent.origin) {
    return userEvent.origin as MessageOrigin;
  }
  // A user-clicked Continue stamps the device on `EventMeta.actor`, which the
  // chip reads for the "You" label. It leaves `origin` empty, so fall back to
  // the actor and the popover matches the chip.
  if ('actor' in userEvent && userEvent.actor) {
    return userEvent.actor as MessageOrigin;
  }
  // Nothing persisted: for a starter only the engine can write, that is an
  // older row rather than an unknown actor, and its shape says why.
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

/** The step that settled a question or permission card: the user's answer or
 *  verdict. `undefined` while the card waits, and `null` for any other turn.
 *  A canceled or superseded question was never answered, so it stays waiting,
 *  as on the card's own status. An MCP consent records no answer at all. */
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

/** Whether the Origin popover says anything the chip does not. A question or
 *  permission card's chip already names the asker, so its popover waits until
 *  someone answered. */
export function originPopoverHasContent(exchange: Exchange): boolean {
  return dividerSettlement(exchange) === null || resolveOrigin(exchange) !== undefined;
}
