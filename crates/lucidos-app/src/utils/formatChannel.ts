export function formatChannel(channel: string): string {
  if (channel === 'claude_code') return 'Claude Code';
  if (channel === 'trigger') return 'Trigger';
  if (channel === 'lucidos' || channel === 'chat') return 'Lucidos';
  if (channel === 'error_unknown_channel') return 'ERROR';
  return channel;
}
