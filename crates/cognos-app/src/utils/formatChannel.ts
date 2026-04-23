export function formatChannel(channel: string): string {
  if (channel === 'claude_code') return 'Claude Code';
  if (channel === 'trigger') return 'Trigger';
  if (channel === 'cognos' || channel === 'chat') return 'Lucidos';
  if (channel === 'error_unknown_channel') return 'ERROR';
  return channel;
}

/**
 * Build a "FROM → TO" route label for thread-level display.
 * Trigger runs route to Lucidos, so collapse "Trigger" → "Lucidos" here.
 */
export function formatThreadRoute(initiator: string, channel: string): string {
  const from = initiator === 'system' ? 'System' : initiator === 'api' ? 'API' : 'User';
  const to = channel === 'trigger' ? 'Lucidos' : formatChannel(channel);
  return `${from} → ${to}`;
}
