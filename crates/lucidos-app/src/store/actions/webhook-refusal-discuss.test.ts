/**
 * Discuss on the refusal bar: what the button hands the Lucidos Agent.
 *
 * Two properties carry this file. The message must quote the DECLARATION, not
 * the sentence the bar draws. An agent told only that deliveries are being
 * thrown away has to ask which hook, which cause and how many. It cannot: the
 * bar is not in its context and nothing else puts it there.
 *
 * And it must never send a switched-off hook's owner to the signature. That
 * wrong turn has already cost one long investigation into HMAC for a hook
 * somebody had simply turned off.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { webhookRefusalNotice } from '../../utils/webhookRefusalNotice';
import type { WebhookRefusal } from '../../api/client';

// `sendSeededPrompt` owns the whole gesture and compose.test.ts covers it: the
// fresh draft it seeds, the forced Lucidos Agent destination, the thread-pane
// reveal, the send and the failure toast. Here it is a seam.
const sendSeededPrompt = vi.fn(async () => true);
vi.mock('./compose', () => ({ sendSeededPrompt }));

const { webhookRefusalDiscussPrompt, discussWebhookRefusal } = await import(
  './webhook-refusal-discuss'
);

/** The source of the action under test, for the two scans below. */
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

/** The other cause, where the delivery reached the verifier and failed it. */
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
    // Which hook, and how the engine classified the run.
    expect(prompt).toContain('GitHub workflow runs');
    expect(prompt).toContain('`disabled`');
    // How many, for how long, and from when. The count is the evidence, and
    // the age is what turns a warning into an incident.
    expect(prompt).toContain('42');
    expect(prompt).toContain('18 days');
    expect(prompt).toContain('Refusal run started: 2026-09-02T04:41:06Z');
  });

  it('reads the age off the span the bar drew, never a clock', () => {
    // `currentWebhookRefusals` advanced `refusing_secs` from the engine's own
    // reading. Parsing `refusing_since` here would subtract a server instant
    // from a local one, which ADR 0053 forbids.
    const source = actionSource('./webhook-refusal-discuss.ts');
    for (const clock of ['Date.now', 'Date.parse', 'new Date']) {
      expect(source, `${clock} would read this machine's clock`).not.toContain(clock);
    }
    // So the phrase tracks that span and nothing else. A second reading, a
    // fortnight on, says a fortnight.
    expect(webhookRefusalDiscussPrompt(refusal(), null)).toContain('18 days');
    expect(webhookRefusalDiscussPrompt(refusal({ refusing_secs: 1_209_600 }), null))
      .toContain('14 days');
  });

  it('counts the hooks the bar is not speaking for', () => {
    // The bar draws that clause and the agent cannot see it. Several hooks
    // failing at once is the evidence that one rotated secret broke them all,
    // and it is the fact a one-hook message would lose.
    expect(webhookRefusalDiscussPrompt(refusal(), '2 other webhooks are too.'))
      .toContain('2 other webhooks are too.');
    // Silent for the ordinary case of one, exactly as the bar is.
    expect(webhookRefusalDiscussPrompt(refusal(), null)).not.toContain('other webhook');
  });

  it('never sends a switched-off hook to the signature', () => {
    // The whole lesson of the 18-day outage. Nothing was read, so the secret
    // and the signature config are both irrelevant, and saying otherwise sends
    // the reader somewhere there is nothing to find.
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
    // A record saying `verification` for a hook that is off. The bullet read
    // the stored value and the sentence read the flag, so the agent was handed
    // both causes at once and could follow either.
    const prompt = webhookRefusalDiscussPrompt(verifying({ enabled: false }), null);
    expect(prompt).toContain('`disabled`');
    expect(prompt).not.toContain('`verification`');
    expect(prompt).toContain('switched off');
  });

  it('names the reason tally with its counts, which is the evidence', () => {
    // The breakdown survives a diagnostic probe, which is what the tally was
    // added for. An agent cannot tell a missing header from a wrong digest
    // without it, and the two have different fixes.
    const prompt = webhookRefusalDiscussPrompt(verifying(), null);
    expect(prompt).toContain('no readable signature (40)');
    expect(prompt).toContain('the signature did not match (2)');
  });

  it('states that tally once, not twice', () => {
    // It rides in on the notice, which names every reason with its count. A
    // bullet beside that sentence would print the same evidence twice.
    const prompt = webhookRefusalDiscussPrompt(verifying(), null);
    expect(prompt.match(/no readable signature \(40\)/g)).toHaveLength(1);
  });

  it('states the notice, from the table the bar and the Webhooks row read', () => {
    // One voice on the cause. A second sentence written here would be the one
    // that drifts when the table is reworded.
    const notice = webhookRefusalNotice(verifying());
    const prompt = webhookRefusalDiscussPrompt(verifying(), null);
    expect(prompt).toContain(notice.title);
    expect(prompt).toContain(notice.detail);
  });

  it('quotes the whole block, blank lines included', () => {
    // A blank line takes a bare `>`, so the facts stay one quote instead of
    // splitting and leaving the closing sentence outside it.
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

  // Nothing renders the bar in this suite, so without a scan the button could
  // be deleted and these tests would stay green. Same guard the ingress
  // Discuss uses, for the same reason.
  it('is reachable: the bar wires a button to this action', () => {
    const source = actionSource('../../components/layout/WebhookRefusalBanner.tsx');
    expect(source).toContain('void discussWebhookRefusal(refusal!, others)');
    // The second argument is the empty focus function: the wrapper is kept for
    // its touch dedup, and Discuss must not raise a keyboard over the reply.
    expect(source).toContain('composeHandlers(props.onDiscuss, () => {})');
  });
});
