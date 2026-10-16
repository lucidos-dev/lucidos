import type { ControlSendResult } from '../../store/actions/chat-claude-code';

/** The toast for a control pick sent to a live session. It names the half
 *  that waits for the next turn, so a delayed change never reads as applied.
 *  `tier` is the effort sent with a *model selection*, `null` when none was. */
export function controlPickToast(
  label: string,
  pick: string,
  result: ControlSendResult,
  tier: ControlSendResult | null,
): string {
  const picked = `${label}: ${pick}`;
  if (result === 'next-turn') return `${picked}. Applies from the next turn`;
  if (tier === 'next-turn') return `${picked}. Effort applies from the next turn`;
  return picked;
}
