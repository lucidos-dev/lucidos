import { describe, expect, it } from 'vitest';
import type { ChatGptPlanStatus } from '../../api/client';
import { chatGptPlanView } from './chatgptPlanView';

function status(overrides: Partial<ChatGptPlanStatus> = {}): ChatGptPlanStatus {
  return {
    account: null,
    sign_in_again: false,
    registered: false,
    sign_in: { state: 'idle' },
    paused_until: null,
    pause_remaining_ms: null,
    ...overrides,
  };
}

const account = { email: 'user@example.com', signed_in_at: '2026-01-01T00:00:00Z' };
const pending = { state: 'pending' as const, expires_at: '2026-01-01T00:10:00Z' };
const failed = { state: 'failed' as const, error: 'access denied' };

describe('chatGptPlanView', () => {
  it('offers a first sign-in with no notice', () => {
    expect(chatGptPlanView(status())).toEqual({ kind: 'signed-out', notice: null, registered: false });
  });

  it('offers another account once a registration is kept', () => {
    expect(chatGptPlanView(status({ registered: true }))).toEqual({
      kind: 'signed-out', notice: null, registered: true,
    });
    expect(chatGptPlanView(status({ registered: true, sign_in: failed }))).toMatchObject({
      registered: true,
    });
  });

  it('shows the stored account, with no pause', () => {
    expect(chatGptPlanView(status({ account }))).toEqual({
      kind: 'signed-in', account, pausedUntil: null, pauseRemainingMs: null,
    });
  });

  it('carries the limit pause on a stored account', () => {
    const pausedUntil = '2026-01-01T00:20:00Z';
    const pause = { paused_until: pausedUntil, pause_remaining_ms: 60_000 };
    expect(chatGptPlanView(status({ account, ...pause }))).toMatchObject({
      kind: 'signed-in', pausedUntil, pauseRemainingMs: 60_000,
    });
  });

  it('puts an attempt in progress ahead of a stored account', () => {
    expect(chatGptPlanView(status({ account, sign_in: pending }))).toEqual({
      kind: 'pending', expiresAt: pending.expires_at, signedIn: true,
    });
  });

  it('marks an attempt with no stored account as safe to cancel', () => {
    expect(chatGptPlanView(status({ sign_in: pending }))).toMatchObject({ signedIn: false });
  });

  it('asks to sign in again after OpenAI refused the last sign-in', () => {
    expect(chatGptPlanView(status({ sign_in_again: true }))).toEqual({
      kind: 'signed-out', notice: { kind: 'sign-in-again' }, registered: false,
    });
  });

  it('ranks a failed attempt over the older sign-in-again', () => {
    expect(chatGptPlanView(status({ sign_in_again: true, sign_in: failed }))).toEqual({
      kind: 'signed-out', notice: { kind: 'failed', error: failed.error }, registered: false,
    });
  });

  it('keeps a stored account through a failed attempt', () => {
    expect(chatGptPlanView(status({ account, sign_in: failed }))).toMatchObject({ kind: 'signed-in' });
  });
});
