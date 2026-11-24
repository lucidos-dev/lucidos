import { describe, expect, it } from 'vitest';
import {
  PEER_CLIENT_BANNER_HEIGHT_VAR,
  PEER_CLIENT_UNAVAILABLE_META,
  peerClientBannerBody,
  unavailablePeerClient,
} from '../PeerClientBanner';
import { SANDBOX_BANNER_HEIGHT_VAR } from '../AgentSandboxBanner';
import { findByClass, findByType, textOf } from './vnodeWalk';

function docWith(meta: string | null): Pick<Document, 'querySelector'> {
  return {
    querySelector: ((selector: string) =>
      meta !== null && selector === `meta[name="${PEER_CLIENT_UNAVAILABLE_META}"]`
        ? { getAttribute: (name: string) => (name === 'content' ? meta : null) }
        : null) as Document['querySelector'],
  };
}

describe('the peer client banner', () => {
  it('reads the peer the gateway could not serve from its meta', () => {
    expect(unavailablePeerClient(docWith('cloud'))).toBe('cloud');
    expect(unavailablePeerClient(docWith(null))).toBeNull();
    expect(unavailablePeerClient(undefined)).toBeNull();
  });

  it('names the peer and offers no dismiss', () => {
    const tree = peerClientBannerBody({ layout: 'desktop', peer: 'cloud' });
    expect(findByClass(tree, 'peer-client-banner')).toHaveLength(1);
    expect(textOf(tree)).toContain('"cloud"');
    expect(findByType(tree, 'button')).toHaveLength(0);
  });

  it('renders nothing when the peer client was served', () => {
    expect(peerClientBannerBody({ layout: 'desktop', peer: null })).toBeNull();
  });

  it('reserves its own height', () => {
    expect(PEER_CLIENT_BANNER_HEIGHT_VAR).not.toBe(SANDBOX_BANNER_HEIGHT_VAR);
  });
});
