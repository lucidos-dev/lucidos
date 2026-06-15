import type { CodingAgent } from '../api/types';

export function formatChannel(channel: string): string {
  if (channel === 'claude_code') return 'Claude Code';
  if (channel === 'trigger') return 'Trigger';
  if (channel === 'lucidos' || channel === 'chat') return 'Lucidos';
  if (channel === 'error_unknown_channel') return 'ERROR';
  return channel;
}

/** Backend-aware label for a thread row's channel tag. Every coding-agent
 *  thread carries the `claude_code` channel (kept for backward compat), so the
 *  real backend lives on `codingAgent` — a Codex thread must read "Codex", not
 *  "Claude Code". All other channels fall through to {@link formatChannel}.
 *  Absent/legacy `codingAgent` defaults to Claude Code. */
export function formatThreadChannelLabel(channel: string, codingAgent?: CodingAgent | null): string {
  if (channel === 'claude_code' && codingAgent === 'codex') return 'Codex';
  return formatChannel(channel);
}
