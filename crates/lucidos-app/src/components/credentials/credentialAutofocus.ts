import type { AuthType } from '../../store/types';

/** Which input the credential form should auto-focus on mount.
 *
 *  Returns `null` on mobile because iOS doesn't open the on-screen keyboard
 *  for a programmatic `.focus()` (no user gesture). The focusin event still
 *  fires, which makes `useHideOnScroll` flip `keyboardOpen=true` and translate
 *  the app header off-screen — but no keyboard appears, so the form ends up
 *  with its content sitting under the iOS status bar with no visible header. */
export function pickCredentialAutofocus(
  authType: AuthType,
  mobile: boolean,
): 'username' | 'clientId' | 'authValue' | null {
  if (mobile) return null;
  if (authType === 'password') return 'username';
  if (authType === 'oauth_client') return 'clientId';
  return 'authValue';
}
