import { describe, it, expect, beforeEach } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { confirmWebhookDeletion } from '../WebhooksPage';
import { confirmState } from '../../../store/store';
import type { Webhook } from '../../../api/client';

function hook(over: Partial<Webhook> = {}): Webhook {
  return {
    id: '11111111-1111-4111-8111-111111111111',
    name: 'deploys',
    event_type: 'DeployFinished',
    enabled: true,
    signed: false,
    hmac: null,
    dedupe: null,
    headers: [],
    created_at: '2026-08-01T09:00:00Z',
    last_accepted_at: null,
    last_refused_at: null,
    last_refusal_reason: null,
    delivery_path: '/hooks/11111111-1111-4111-8111-111111111111',
    ...over,
  };
}

describe('deleting a webhook asks first', () => {
  beforeEach(() => {
    confirmState.value = { visible: false, message: '', okLabel: 'Delete' };
  });

  it('raises a danger confirm naming the hook, and waits', () => {
    const answer = confirmWebhookDeletion(hook());
    const shown = confirmState.value;
    expect(shown.visible).toBe(true);
    expect(shown.message).toContain('deploys');
    expect(shown.okLabel).toBe('Delete');
    expect(shown.variant).toBe('danger');
    shown.resolve?.(false);
    return expect(answer).resolves.toBe(false);
  });

  it('says what the sender loses, in its own paragraph', () => {
    void confirmWebhookDeletion(hook());
    const [question, consequence] = confirmState.value.message.split('\n\n');
    expect(question).toBe('Delete the webhook "deploys"?');
    // The path carries the hook's id, so a replacement is a different URL
    // rather than this one restored. That is the part a sender pays for.
    expect(consequence).toContain('stops answering');
    expect(consequence).toContain('different path');
    confirmState.value.resolve?.(false);
  });

  it('resolves true when the user says Delete', async () => {
    const answer = confirmWebhookDeletion(hook());
    confirmState.value.resolve?.(true);
    expect(await answer).toBe(true);
  });
});

describe('the Delete button cannot skip the confirm', () => {
  const here = dirname(fileURLToPath(import.meta.url));
  const src: string = readFileSync(resolve(here, '../WebhooksPage.tsx'), 'utf8');

  it('gates the one delete call on the confirm', () => {
    const gate = src.indexOf('await confirmWebhookDeletion(hook)');
    const call = src.indexOf('await deleteWebhook(');
    expect(gate, 'the delete path lost its confirm').toBeGreaterThan(-1);
    expect(call, 'the delete call moved ahead of its confirm').toBeGreaterThan(gate);
    expect(
      src.split('deleteWebhook(hook.id)').length - 1,
      'a second delete call would be unguarded',
    ).toBe(1);
  });
});
