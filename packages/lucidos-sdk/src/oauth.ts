import { request } from './_fetch';
import { assertString } from './_validate';

export interface AccessToken {
  accessToken: string;
  expiresAt: Date | null;
}

interface AccessTokenWire {
  access_token: string;
  expires_at: string | null;
}

export const oauth = {
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
