import { postCommandConsent, postMcpConsent, postMcpPermissionConsent } from '../../api/client';
import type { PersistScope } from '../thread-events';
import { scrollToBottom } from '../../components/chat/scrollState';

/** Resolve an inline permission card and force the transcript to the bottom.
 *
 *  Forcing the scroll (not the conditional `preserveAtBottom`) means that even
 *  when the user scrolled up to read history while the card was pending,
 *  resolving it re-activates auto-scroll so the agent's resumed stream tails the
 *  bottom — the same contract `answerThreadQuestion` gives the question card.
 *
 *  Throws on a failed POST; the card's optimistic `decide` rolls back its
 *  pending state and toasts. Used by all three in-thread permission cards
 *  (coding-agent, command guard, chat MCP). */
async function resolveAndPin(
  consent: (id: string, allowed: boolean, persist?: PersistScope) => Promise<void>,
  requestId: string,
  allowed: boolean,
  persist?: PersistScope,
): Promise<void> {
  scrollToBottom();
  await consent(requestId, allowed, persist);
}

/** Resolve a coding-agent permission card (CC's MCP prompt / Codex's app-server
 *  approval bridge). */
export function resolveCodingAgentPermission(
  requestId: string,
  allowed: boolean,
  persist?: PersistScope,
): Promise<void> {
  return resolveAndPin(postMcpConsent, requestId, allowed, persist);
}

/** Resolve a chat command-guard permission card (ADR 0002). */
export function resolveCommandPermission(
  requestId: string,
  allowed: boolean,
  persist?: PersistScope,
): Promise<void> {
  return resolveAndPin(postCommandConsent, requestId, allowed, persist);
}

/** Resolve a chat MCP permission card (MCP server tool call). */
export function resolveMcpPermission(
  requestId: string,
  allowed: boolean,
  persist?: PersistScope,
): Promise<void> {
  return resolveAndPin(postMcpPermissionConsent, requestId, allowed, persist);
}
