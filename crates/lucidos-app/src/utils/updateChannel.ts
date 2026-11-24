import type { ReleaseCheck } from '../api/client/control';

export function updateChannelLabel(channel: string): string {
  if (channel.startsWith('custom:')) return `Custom: ${channel.slice(7)}`;
  switch (channel) {
    case 'stable': return 'Stable';
    case 'beta': return 'Beta';
    case 'local': return 'Local';
    default: return channel;
  }
}

export function withUpdateChannel(version: string, channel?: string): string {
  return channel ? `${version} · ${updateChannelLabel(channel)}` : version;
}

export function hasUpdateChannelChoice(check: ReleaseCheck | null): boolean {
  return !!check?.channels?.some(c => c.channel !== 'stable' && c.channel !== 'nightly' && c.available);
}
