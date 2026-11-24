import type { CodingAgent } from '../api/types';

export function formatChannel(channel: string): string {
  if (channel === 'claude_code') return 'Claude Code';
  if (channel === 'trigger') return 'Trigger';
  if (channel === 'lucidos' || channel === 'chat') return 'Lucidos';
  if (channel === 'error_unknown_channel') return 'ERROR';
  return channel;
}

export function formatThreadChannelLabel(channel: string, codingAgent?: CodingAgent | null): string {
  if (channel === 'claude_code' && codingAgent === 'codex') return 'Codex';
  if (channel === 'lucidos' || channel === 'chat') return 'Lucidos Agent';
  return formatChannel(channel);
}
