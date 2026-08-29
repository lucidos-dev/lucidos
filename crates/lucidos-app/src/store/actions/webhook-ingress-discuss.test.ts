/**
 * Discuss on the ingress bar: what the button hands the Lucidos Agent.
 *
 * One property carries this file. The message must quote the DECLARATION, not
 * the sentence the bar draws. An agent given only "IPv4 is not arriving" has to
 * ask which hook, which host and what each address answered, and it cannot: the
 * bar is not in its context and nothing else puts it there.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import type { WebhookIngressOutage } from '../../api/client';

// `sendSeededPrompt` owns the whole gesture and compose.test.ts covers it: the
// confirm before replacing a draft, the forced Lucidos Agent destination, the
// thread-pane reveal, the send and the failure toast. Here it is a seam.
const sendSeededPrompt = vi.fn(async () => true);
vi.mock('./compose', () => ({ sendSeededPrompt }));

const { webhookIngressDiscussPrompt, discussWebhookIngress } = await import(
  './webhook-ingress-discuss'
);

function outage(over: Partial<WebhookIngressOutage> = {}): WebhookIngressOutage {
  return {
    webhook_name: 'github-ci',
    host: 'node.tailnet.ts.net',
    port: 8443,
    families: ['ipv4'],
    addresses: [
      {
        address: '203.0.113.7',
        family: 'ipv4',
        stage: 'ingress-unreachable',
        status: null,
        detail: 'could not connect: tls handshake eof',
      },
      { address: '2001:db8::1', family: 'ipv6', stage: 'healthy', status: 401, detail: null },
    ],
    down_since: '2026-08-26T22:10:00Z',
    down_secs: 28_800,
    ...over,
  };
}

describe('webhookIngressDiscussPrompt', () => {
  it('carries every fact a reader needs to reason about the outage', () => {
    const prompt = webhookIngressDiscussPrompt(outage());
    // The hook the probe knocked on, and the public path it knocked at.
    expect(prompt).toContain('github-ci');
    expect(prompt).toContain('node.tailnet.ts.net:8443');
    // Which families, and for how long. The age is the fact that turns a
    // warning into an incident, and the bar is the only place it is measured.
    expect(prompt).toContain('IPv4');
    expect(prompt).toContain('8 hours');
    expect(prompt).toContain('2026-08-26T22:10:00Z');
  });

  it('names what each address answered, not just that a family is down', () => {
    // The stage IS the diagnosis. A handshake that died is a different fault
    // from a 502, and the agent cannot tell them apart from the family alone.
    const prompt = webhookIngressDiscussPrompt(outage());
    expect(prompt).toContain('`203.0.113.7` (IPv4): ingress-unreachable');
    expect(prompt).toContain('could not connect: tls handshake eof');
    // The healthy family is quoted too: it is what proves the chain behind the
    // ingress is fine, which is most of the diagnosis.
    expect(prompt).toContain('`2001:db8::1` (IPv6): healthy, HTTP 401');
  });

  it('quotes the whole block, blank lines included', () => {
    // A blank line takes a bare `>`, so the facts stay one quote instead of
    // splitting and leaving the addresses outside it.
    const prompt = webhookIngressDiscussPrompt(outage());
    const [lead, ...quoted] = prompt.split('\n');
    expect(lead).toBe("Let's discuss this webhook ingress outage:");
    expect(quoted[0]).toBe('');
    for (const line of quoted.slice(1)) expect(line.startsWith('>')).toBe(true);
  });

  it('says nothing about addresses when the declaration carries none', () => {
    // A declaration written before the engine read that field. The heading and
    // the facts still stand; an empty "What each address answered:" would read
    // as a probe that found nothing.
    const prompt = webhookIngressDiscussPrompt(outage({ addresses: [] }));
    expect(prompt).not.toContain('What each address answered');
    expect(prompt).toContain('node.tailnet.ts.net:8443');
  });

  it('names both families when both are down', () => {
    const prompt = webhookIngressDiscussPrompt(outage({ families: ['ipv4', 'ipv6'] }));
    expect(prompt).toContain('degraded over IPv4 and IPv6');
  });
});

describe('discussWebhookIngress', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    sendSeededPrompt.mockResolvedValue(true);
  });

  it('sends the seeded message, naming the gesture for the failure toast', async () => {
    const o = outage();
    await discussWebhookIngress(o);
    expect(sendSeededPrompt).toHaveBeenCalledWith(
      webhookIngressDiscussPrompt(o),
      'start a discussion about the webhook ingress outage',
    );
  });

  it('swallows nothing on a declined or failed send: the seam toasts', async () => {
    sendSeededPrompt.mockResolvedValue(false);
    await expect(discussWebhookIngress(outage())).resolves.toBeUndefined();
  });

  // Nothing renders the bar in this suite, so without a scan the button could
  // be deleted and these tests would stay green. Same guard the notification
  // Discuss uses, for the same reason.
  it('is reachable: the bar wires a button to this action', () => {
    const here: string = dirname(fileURLToPath(import.meta.url));
    const source = readFileSync(
      resolve(here, '../../components/layout/IngressBanner.tsx'),
      'utf-8',
    );
    expect(source).toContain('void discussWebhookIngress(outage!)');
    // The second argument is the empty focus function: the wrapper is kept for
    // its touch dedup, and Discuss must not raise a keyboard over the reply.
    expect(source).toContain('composeHandlers(props.onDiscuss, () => {})');
  });
});
