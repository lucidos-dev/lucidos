import { request } from './_fetch';
import { assertString } from './_validate';

export interface AccessToken {
  /** The bearer token to hand to an in-browser SDK (e.g. Spotify Web Playback). */
  accessToken: string;
  /** When the upstream provider says this token expires, or `null` if the
   *  provider didn't include an expiry. */
  expiresAt: Date | null;
}

interface AccessTokenWire {
  access_token: string;
  expires_at: string | null;
}

export const oauth = {
  /**
   * Fetch a short-lived OAuth access token for `provider` from the engine.
   * The engine looks up the connected account, refreshes the token if it's
   * expired or expiring within 60s, and returns ONLY the access token —
   * the refresh token never leaves the engine.
   *
   * Use this for in-browser SDKs that need a bearer token (e.g. the
   * Spotify Web Playback SDK's `getOAuthToken` callback). Re-call from
   * the SDK's callback when the token nears expiry.
   *
   * For ordinary HTTP calls to the upstream API, prefer
   * `lucidos.proxy(<provider>).fetch(...)` — the engine attaches the
   * bearer header server-side and the iframe never sees the token.
   */
  async getAccessToken(provider: string): Promise<AccessToken> {
    assertString('provider', provider);
    const wire = await request<AccessTokenWire>(
      `/oauth/${encodeURIComponent(provider)}/access-token`,
    );
    return {
      accessToken: wire.access_token,
      expiresAt: wire.expires_at ? new Date(wire.expires_at) : null,
    };
  },
};
