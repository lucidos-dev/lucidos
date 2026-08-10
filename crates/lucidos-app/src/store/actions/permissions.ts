import { postCommandConsent, postMcpConsent, postMcpPermissionConsent } from '../../api/client';
import type { PersistScope } from '../thread-events';

/* Resolving an in-thread permission card posts the consent and nothing else.
 *
 * All three used to route through a `resolveAndPin` helper that forced the
 * transcript to the bottom first, on the reasoning that resolving a card
 * resumes the agent's stream and the reader would want to tail it. Forcing it is
 * the app deciding where the reader looks, so it is gone (see the header of
 * `components/chat/scrollState.ts`): a reader following nothing keeps their
 * position while the resumed stream grows below them, and the down chevron is
 * how they follow it. A reader who DID ask to ride the live edge is still taken
 * there, from the button in `PermissionCard` rather than from here, because that
 * is where the reader's own tap lives and this module is also the transport for
 * a decision nobody watched happen. (Naming that entry point here would trip the
 * source scan in `scroll-follow-the-live-edge.test.ts`, which is deliberately
 * wide: a store action that so much as mentions one is reaching for the live
 * edge.) With the scroll removed from the transport the helper was a bare
 * call-through, so each card calls its own endpoint directly.
 *
 * Each throws on a failed POST; the card's optimistic `decide` rolls back its
 * pending state and toasts.
 */

/** Resolve a coding-agent permission card (CC's MCP prompt / Codex's app-server
 *  approval bridge). */
export function resolveCodingAgentPermission(
  requestId: string,
  allowed: boolean,
  persist?: PersistScope,
): Promise<void> {
  return postMcpConsent(requestId, allowed, persist);
}

/** Resolve a chat command-guard permission card (ADR 0002). */
export function resolveCommandPermission(
  requestId: string,
  allowed: boolean,
  persist?: PersistScope,
): Promise<void> {
  return postCommandConsent(requestId, allowed, persist);
}

/** Resolve a chat MCP permission card (MCP server tool call). */
export function resolveMcpPermission(
  requestId: string,
  allowed: boolean,
  persist?: PersistScope,
): Promise<void> {
  return postMcpPermissionConsent(requestId, allowed, persist);
}
