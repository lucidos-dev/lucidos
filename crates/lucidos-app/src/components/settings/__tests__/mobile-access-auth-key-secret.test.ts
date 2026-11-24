import { describe, it, expect } from 'vitest';
import PAGE from '../MobileAccessPage.tsx?raw';

function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^\\:])\/\/.*$/gm, '$1');
}

const page = stripComments(PAGE);

function authKeyField(): string {
  const matches = page.match(/<input\b[^>]*value=\{authKey\}[^>]*>/g) ?? [];
  expect(matches, 'expected exactly one input bound to authKey').toHaveLength(1);
  const [field] = matches;
  if (field === undefined) throw new Error('no input is bound to authKey');
  return field;
}

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

  it('keeps no clear on the success path alone', () => {
    const onUp = signInHandler();
    expect(onUp.slice(0, onUp.indexOf('finally'))).not.toMatch(/setAuthKey\(''\)/);
  });
});
