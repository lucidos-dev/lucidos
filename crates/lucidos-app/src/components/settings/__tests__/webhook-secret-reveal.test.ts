/**
 * A webhook's bearer token is shown once and never again, since only its
 * digest is stored. The toast that reveals it must wait for the user, and must
 * not leave on a timer before they can copy it.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { revealSecret } from '../WebhooksPage';
import { toasts } from '../../../store/store';
import type { WebhookWithToken } from '../../../api/client';

const CREATED = { id: 'h1', name: 'Deploys', token: 'tok-123' } as unknown as WebhookWithToken;

describe('the secret-reveal toast', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    toasts.value = [];
  });

  afterEach(() => {
    vi.useRealTimers();
    toasts.value = [];
  });

  it('stays until the user answers it, with its Copy button', () => {
    revealSecret(CREATED);
    vi.advanceTimersByTime(60_000);
    const toast = toasts.value.find((t) => t.message.includes('tok-123'));
    expect(toast, 'the token toast left on its own').toBeDefined();
    expect(toast?.action?.label).toBe('Copy token');
    expect(toast?.persistent).toBe(true);
  });
});
