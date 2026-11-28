export const PEER_CLIENT_META = 'lucidos-peer-client';

export function peerClient(): string | null {
  if (typeof document === 'undefined') return null;
  const name = document.querySelector(`meta[name="${PEER_CLIENT_META}"]`)?.getAttribute('content');
  return name && /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(name) ? name : null;
}

export function peerPrefix(peer: string | null): string {
  const preview = typeof document !== 'undefined' && document.querySelector('meta[name="lucidos-frontend-preview"]') !== null;
  return `${preview ? 'preview:' : ''}${peer === null ? '' : `peer:${peer}:`}`;
}

export function peerScopedKey(key: string, peer: string | null): string {
  const prefix = peerPrefix(peer);
  return key.startsWith(prefix) ? key : `${prefix}${key}`;
}
