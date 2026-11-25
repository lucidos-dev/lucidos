import { describe, it, expect, beforeEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { webhookRefusalNotice } from '../../utils/webhookRefusalNotice';
import type { WebhookRefusal } from '../../api/client';
import { webhookRefusals } from '../store';
import { currentWebhookRefusals } from './webhookRefusals';

const sendSeededPrompt = vi.fn(async () => true);
vi.mock('./compose', () => ({ sendSeededPrompt }));

const { webhookRefusalDiscussPrompt, discussWebhookRefusal } = await import(
  './webhook-refusal-discuss'
);

function actionSource(relative: string): string {
  const here: string = dirname(fileURLToPath(import.meta.url));
  return readFileSync(resolve(here, relative), 'utf-8');
}

function refusal(over: Partial<WebhookRefusal> = {}): WebhookRefusal {
  return {
    webhook_id: '6f1c0f3e-0000-4000-8000-000000000001',
    webhook_name: 'GitHub workflow runs',
    enabled: false,
    cause: 'disabled',
    refusals: 42,
    reasons: { disabled: 42 },
    refusing_since: '2026-09-02T04:41:06Z',
    refusing_secs: 1_555_200,
    ...over,
  };
}

function verifying(over: Partial<WebhookRefusal> = {}): WebhookRefusal {
  return refusal({
    enabled: true,
    cause: 'verification',
    reasons: { 'signature-missing': 40, 'signature-mismatch': 2 },
    ...over,
  });
}

describe('webhookRefusalDiscussPrompt', () => {
  it('carries every fact a reader needs to reason about the refusal', () => {
    const prompt = webhookRefusalDiscussPrompt(refusal(), null);
    expect(prompt).toContain('GitHub workflow runs');
    expect(prompt).toContain('`disabled`');
    expect(prompt).toContain('42');
    expect(prompt).toContain('18 days');
    expect(prompt).toContain('Refusal run started: 2026-09-02T04:41:06Z');
  });

  it('reads the age off the span the bar drew, never a clock', () => {
    const source = actionSource('./webhook-refusal-discuss.ts');
    for (const clock of ['Date.now', 'Date.parse', 'new Date']) {
      expect(source, `${clock} would read this machine's clock`).not.toContain(clock);
    }
    expect(webhookRefusalDiscussPrompt(refusal(), null)).toContain('18 days');
    expect(webhookRefusalDiscussPrompt(refusal({ refusing_secs: 1_209_600 }), null))
      .toContain('14 days');
  });

  it('counts the hooks the bar is not speaking for', () => {
    expect(webhookRefusalDiscussPrompt(refusal(), '2 other webhooks are too.'))
      .toContain('2 other webhooks are too.');
    expect(webhookRefusalDiscussPrompt(refusal(), null)).not.toContain('other webhook');
  });

  it('never sends a switched-off hook to the signature', () => {
    const prompt = webhookRefusalDiscussPrompt(refusal(), null);
    expect(prompt).toContain('switched off');
    expect(prompt).toContain('Nothing is wrong with the signature or the secret');
    expect(prompt).not.toContain('this is the secret or the signature config');
  });

  it('sends a verification fault to the secret, which is where its fix is', () => {
    const prompt = webhookRefusalDiscussPrompt(verifying(), null);
    expect(prompt).toContain('`verification`');
    expect(prompt).toContain('this is the secret or the signature config');
    expect(prompt).not.toContain('switched off');
  });

  it('names one cause, the one the notice under it speaks from', () => {
    const prompt = webhookRefusalDiscussPrompt(verifying({ enabled: false }), null);
    expect(prompt).toContain('`disabled`');
    expect(prompt).not.toContain('`verification`');
    expect(prompt).toContain('switched off');
  });

  it('names the reason tally with its counts, which is the evidence', () => {
    const prompt = webhookRefusalDiscussPrompt(verifying(), null);
    expect(prompt).toContain('no readable signature (40)');
    expect(prompt).toContain('the signature did not match (2)');
  });

  it('states that tally once, not twice', () => {
    const prompt = webhookRefusalDiscussPrompt(verifying(), null);
    expect(prompt.match(/no readable signature \(40\)/g)).toHaveLength(1);
  });

  it('states the notice, from the table the bar and the Webhooks row read', () => {
    const notice = webhookRefusalNotice(verifying());
    const prompt = webhookRefusalDiscussPrompt(verifying(), null);
    expect(prompt).toContain(notice.title);
    expect(prompt).toContain(notice.detail);
  });

  it('quotes the whole block, blank lines included', () => {
    const prompt = webhookRefusalDiscussPrompt(refusal(), null);
    const [lead, ...quoted] = prompt.split('\n');
    expect(lead).toBe("Let's discuss these refused webhook deliveries:");
    expect(quoted[0]).toBe('');
    for (const line of quoted.slice(1)) expect(line.startsWith('>')).toBe(true);
  });
});

describe('discussWebhookRefusal', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    sendSeededPrompt.mockResolvedValue(true);
  });

  it('sends the seeded message, naming the gesture for the failure toast', async () => {
    const r = refusal();
    await discussWebhookRefusal(r, '1 other webhook is too.');
    expect(sendSeededPrompt).toHaveBeenCalledWith(
      webhookRefusalDiscussPrompt(r, '1 other webhook is too.'),
      'start a discussion about the refused webhook deliveries',
    );
  });

  it('swallows nothing on a declined or failed send: the seam toasts', async () => {
    sendSeededPrompt.mockResolvedValue(false);
    await expect(discussWebhookRefusal(refusal(), null)).resolves.toBeUndefined();
  });

  it('leaves the standing refusal on the bar, since a discussion measures no recovery', async () => {
    const r = refusal();
    webhookRefusals.value = { status: 'loaded', data: { refusals: { refusing: [r] }, receivedAt: Date.now() } };
    await discussWebhookRefusal(r, null);
    expect(currentWebhookRefusals(Date.now()).map((x) => x.webhook_id)).toEqual([r.webhook_id]);
  });

  it('is reachable: the bar wires a button to this action', () => {
    const source = actionSource('../../components/layout/WebhookRefusalBanner.tsx');
    expect(source).toContain('void discussWebhookRefusal(refusal!, others)');
    expect(source).toContain('composeHandlers(props.onDiscuss, () => {})');
  });
});
