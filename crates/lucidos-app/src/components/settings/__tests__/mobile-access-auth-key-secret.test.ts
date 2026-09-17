/**
 * The Tailscale auth key is a SECRET, and is treated as one.
 *
 * A `tskey-auth-…` pre-authorized key enrolls nodes onto the reader's tailnet.
 * It shipped in a plain `type="text"` field, unmasked, while every other secret
 * in settings uses `type="password"` or `SecretInput`. The clear also sat inside
 * the success path. So a rejected key stayed legible for as long as the page was
 * open, under nothing but an error toast.
 *
 * Source-scan rather than a mounted render, for the reason
 * `mobile-access-expose-run.test.ts` gives: `SettingsView` pulls in the whole
 * store, the model registry, OAuth and device state. The page arrives as a
 * `?raw` string so this file needs no Node imports.
 */
import { describe, it, expect } from 'vitest';
import PAGE from '../MobileAccessPage.tsx?raw';

/** Strip comments so the prose explaining a rule can never stand in for it. */
function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^\\:])\/\/.*$/gm, '$1');
}

const page = stripComments(PAGE);

/** The one `<input>` bound to the auth-key state, up to its first `>`. */
function authKeyField(): string {
  const matches = page.match(/<input\b[^>]*value=\{authKey\}[^>]*>/g) ?? [];
  expect(matches, 'expected exactly one input bound to authKey').toHaveLength(1);
  const [field] = matches;
  if (field === undefined) throw new Error('no input is bound to authKey');
  return field;
}

/** `onUp`'s body, from its declaration to the next handler's. */
function signInHandler(): string {
  const start = page.indexOf('const onUp = useCallback');
  const end = page.indexOf('const onServe = useCallback');
  expect(start, 'onUp is gone or renamed').toBeGreaterThan(-1);
  expect(end, 'onServe is gone or renamed').toBeGreaterThan(start);
  return page.slice(start, end);
}

describe('the Tailscale auth-key field', () => {
  it('is masked', () => {
    expect(authKeyField()).toMatch(/type="password"/);
  });

  it('is never a plain-text field', () => {
    expect(authKeyField()).not.toMatch(/type="text"/);
  });
});

describe('the Sign in handler', () => {
  it('drops the key from a `finally`, so a rejected invoke clears it too', () => {
    const onUp = signInHandler();
    const finallyAt = onUp.indexOf('finally');
    expect(finallyAt, 'onUp has no finally block').toBeGreaterThan(-1);
    expect(onUp.slice(finallyAt)).toMatch(/setAuthKey\(''\)/);
  });

  // One clear, in one place. A second inside the `try` would be the shape this
  // replaced, and it reads as covering both outcomes when it covers one.
  it('keeps no clear on the success path alone', () => {
    const onUp = signInHandler();
    expect(onUp.slice(0, onUp.indexOf('finally'))).not.toMatch(/setAuthKey\(''\)/);
  });
});
