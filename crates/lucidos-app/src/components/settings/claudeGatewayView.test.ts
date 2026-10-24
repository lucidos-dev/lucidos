import { describe, expect, it } from 'vitest';
import type { ClaudeGatewayStatus } from '../../api/client';
import { claudeGatewayView, initialGatewayUrl } from './claudeGatewayView';

const GATEWAY = 'https://gateway.example';

function status(overrides: Partial<ClaudeGatewayStatus> = {}): ClaudeGatewayStatus {
  return {
    url: GATEWAY,
    suggested_url: null,
    account: null,
    sign_in_again: false,
    telemetry: false,
    sign_in: { state: 'idle' },
    ...overrides,
  };
}

const account = { issuer: GATEWAY, email: 'user@example.com', signed_in_at: '2026-01-01T00:00:00Z' };

const pending = {
  state: 'pending' as const,
  issuer: GATEWAY,
  user_code: 'ABCD-EFGH',
  verification_uri: `${GATEWAY}/device`,
  verification_uri_complete: `${GATEWAY}/device?user_code=ABCD-EFGH`,
  expires_at: '2026-01-01T00:10:00Z',
};

describe('claudeGatewayView', () => {
  it('shows a first sign-in with no notice', () => {
    expect(claudeGatewayView(status())).toEqual({ kind: 'signed-out', notice: null });
  });

  it('shows the account once signed in', () => {
    expect(claudeGatewayView(status({ account }))).toEqual({
      kind: 'signed-in',
      account,
      telemetry: false,
    });
  });

  it('says when the gateway collects usage telemetry', () => {
    expect(claudeGatewayView(status({ account, telemetry: true }))).toMatchObject({
      kind: 'signed-in',
      telemetry: true,
    });
  });

  it('shows the code and the page carrying it while a sign-in waits', () => {
    expect(claudeGatewayView(status({ account, sign_in: pending }))).toEqual({
      kind: 'pending',
      userCode: 'ABCD-EFGH',
      signInPage: pending.verification_uri_complete,
      expiresAt: pending.expires_at,
      signedIn: true,
    });
  });

  it('falls back to the plain page when the gateway gave no complete one', () => {
    const view = claudeGatewayView(
      status({ sign_in: { ...pending, verification_uri_complete: null } }),
    );
    expect(view).toMatchObject({
      kind: 'pending',
      signInPage: pending.verification_uri,
      signedIn: false,
    });
  });

  it('says to sign in again after the gateway rejected the sign-in', () => {
    expect(claudeGatewayView(status({ sign_in_again: true }))).toEqual({
      kind: 'signed-out',
      notice: { kind: 'sign-in-again' },
    });
  });

  it('names why the last sign-in failed, over the older notice', () => {
    const failed = { state: 'failed' as const, issuer: GATEWAY, error: 'the code expired' };
    expect(claudeGatewayView(status({ sign_in_again: true, sign_in: failed }))).toEqual({
      kind: 'signed-out',
      notice: { kind: 'failed', error: 'the code expired' },
    });
  });

  it('keeps the account when a sign-in failed after it', () => {
    const failed = { state: 'failed' as const, issuer: GATEWAY, error: 'declined' };
    expect(claudeGatewayView(status({ account, sign_in: failed }))).toEqual({
      kind: 'signed-in',
      account,
      telemetry: false,
    });
  });
});

describe('initialGatewayUrl', () => {
  it('prefers the stored URL over the suggestion', () => {
    expect(initialGatewayUrl(GATEWAY, 'https://other.example')).toBe(GATEWAY);
  });

  it('offers the managed suggestion when nothing is stored', () => {
    expect(initialGatewayUrl('', GATEWAY)).toBe(GATEWAY);
  });

  it('is empty with neither', () => {
    expect(initialGatewayUrl('', null)).toBe('');
  });
});
