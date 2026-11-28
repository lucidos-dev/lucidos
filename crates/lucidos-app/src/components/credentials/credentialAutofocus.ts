import type { AuthType } from '../../store/types';

export function pickCredentialAutofocus(
  authType: AuthType,
  mobile: boolean,
): 'username' | 'clientId' | 'authValue' | null {
  if (mobile) return null;
  if (authType === 'password') return 'username';
  if (authType === 'oauth_client') return 'clientId';
  return 'authValue';
}
