import type { CodingAgent } from '../api/types';

export function formatChannel(channel: string): string {
  if (channel === 'claude_code') return 'Claude Code';
  if (channel === 'trigger') return 'Trigger';
  if (channel === 'lucidos' || channel === 'chat') return 'Lucidos';
  if (channel === 'error_unknown_channel') return 'ERROR';
  return channel;
}

/** Backend-aware name of what drives a thread: the drawer row mark's tooltip
 *  and accessible name. Every coding-agent thread carries the `claude_code`
 *  channel, so the backend lives on `codingAgent`: a Codex thread reads
 *  "Codex", not "Claude Code". Plain chat threads read "Lucidos Agent", the LLM
 *  driving them. Other channels fall through to {@link formatChannel}, which
 *  says "Lucidos" where a shorter word is wanted. An absent `codingAgent` reads
 *  as Claude Code. */
export function formatThreadChannelLabel(channel: string, codingAgent?: CodingAgent | null): string {
  if (channel === 'claude_code' && codingAgent === 'codex') return 'Codex';
  if (channel === 'lucidos' || channel === 'chat') return 'Lucidos Agent';
  return formatChannel(channel);
}
