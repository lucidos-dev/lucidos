/**
 * The refusal wording table, read by the app bar and by the matching Webhooks
 * row.
 *
 * One property carries the module: a SWITCHED-OFF hook never reads like a
 * signature problem. The real fault cost a long investigation into HMAC for a
 * hook somebody had simply turned off. Wording that blurred the two would
 * repeat that at the last layer, after the engine went to the trouble of
 * telling them apart.
 */
import { describe, it, expect } from 'vitest';

import {
  refusalReasonsPhrase,
  webhookRefusalNotice,
  webhookRefusalRowLine,
} from './webhookRefusalNotice';
import type { WebhookRefusal } from '../api/client';

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
    reasons: { 'signature-mismatch': 41, 'signature-missing': 1 },
    refusing_secs: 97_200,
    ...over,
  });
}

/** A run of nothing but missing signatures, as unsigned probes leave. */
function unsigned(over: Partial<WebhookRefusal> = {}): WebhookRefusal {
  return verifying({
    refusals: 3,
    reasons: { 'signature-missing': 3 },
    refusing_secs: 7_200,
    ...over,
  });
}

describe('webhookRefusalNotice', () => {
  it('says a switched-off hook is switched off, in as many words', () => {
    // The highest-value alarm in the feature. Eighteen days of deliveries were
    // thrown away by a hook a browser click had disabled, and nothing said so.
    const notice = webhookRefusalNotice(refusal());
    expect(notice.title).toContain('GitHub workflow runs');
    expect(notice.title).toContain('switched off');
    expect(notice.title).toContain('thrown away');
  });

  it('sends a switched-off hook nowhere near the signature', () => {
    // The wrong turn this fault already cost once. A disabled hook is refused
    // before the body is read, so there is nothing to find in the HMAC config.
    const detail = webhookRefusalNotice(refusal()).detail;
    expect(detail).toContain('Nothing is wrong with the signature or the secret');
    expect(detail).toContain('Switch it back on');
  });

  it('sends a verification fault to the secret, and never to the switch', () => {
    const notice = webhookRefusalNotice(verifying());
    expect(notice.title).toContain('refusing every delivery');
    expect(notice.title).not.toContain('switched off');
    expect(notice.detail).toContain('the secret or the signature config');
    expect(notice.detail).not.toContain('Switch it back on');
  });

  it('never sends a hook that is OFF at the secret, whatever the cause says', () => {
    // A run forms while the hook is on, then somebody switches the hook off.
    // The stored cause still says verification, and the live flag says the
    // delivery was thrown away before anything read it.
    //
    // `judge` settles this in the engine, and this is the second layer. The
    // verification words tell the reader to rotate or re-point the hook, and
    // re-pointing replaces the whole config object and drops the secret. So
    // the wrong words turn one click into a multi-day outage.
    const off = verifying({ enabled: false });
    const notice = webhookRefusalNotice(off);
    expect(notice.detail).not.toContain('the secret or the signature config');
    expect(notice.title).toContain('switched off');
    expect(notice.detail).toContain('Nothing is wrong with the signature or the secret');
    // The Webhooks row reads the same flag, so the two surfaces agree.
    expect(webhookRefusalRowLine(off)).toContain('switched off');
  });

  it('never sends an unsigned run at the secret', () => {
    // A sender with a secret set signs every delivery. A run with no signature
    // at all is weak evidence against the secret. Rotating it is the one step
    // that can break a working hook.
    const notice = webhookRefusalNotice(unsigned());
    expect(notice.title).toContain('unsigned requests');
    expect(notice.detail).toContain('none of them carried a readable signature');
    expect(notice.detail).toContain('may not be from your sender');
    expect(notice.detail).toContain('do not rotate it');
    expect(notice.detail).not.toContain('the secret or the signature config');
    // The headline already names the one reason, so the tally is not repeated.
    expect(notice.detail).not.toContain('no readable signature (3)');
  });

  it('keeps the secret words when one real mismatch sits in the run', () => {
    // A stray probe adds a missing signature beside real failures. It must not
    // soften the reading of the forty-one that failed the check.
    const notice = webhookRefusalNotice(verifying());
    expect(notice.title).not.toContain('unsigned');
    expect(notice.detail).toContain('the secret or the signature config');
  });

  it('says switched off for an unsigned run on a hook that is off', () => {
    const notice = webhookRefusalNotice(unsigned({ enabled: false }));
    expect(notice.title).toContain('switched off');
    expect(notice.title).not.toContain('unsigned');
  });

  it('names how many were lost and over how long', () => {
    const detail = webhookRefusalNotice(refusal()).detail;
    expect(detail).toContain('42 deliveries');
    expect(detail).toContain('18 days');
  });

  it('counts one delivery as one, verb and all', () => {
    // A switched-off hook declares on a single refusal, because nothing was
    // read and the loss is certain. So one is the ORDINARY reading of the
    // highest-value alarm here, not an edge. Get it wrong and most users of
    // that alarm read "1 delivery have arrived".
    for (const one of [refusal({ refusals: 1 }), verifying({ refusals: 1 })]) {
      const detail = webhookRefusalNotice(one).detail;
      expect(detail).toContain('1 delivery has arrived');
      expect(detail).not.toContain('1 deliveries');
      expect(detail).not.toContain('1 delivery have');
    }
    expect(webhookRefusalNotice(refusal({ refusals: 2 })).detail)
      .toContain('2 deliveries have arrived');
  });

  it('states how often the engine looks again', () => {
    expect(webhookRefusalNotice(refusal()).detail).toContain('Rechecked every 15 minutes.');
    expect(webhookRefusalNotice(verifying()).detail).toContain('Rechecked every 15 minutes.');
  });
});

describe('refusalReasonsPhrase', () => {
  it('ranks by count and reads each reason for a person', () => {
    expect(refusalReasonsPhrase(verifying())).toBe(
      'the signature did not match (41), no readable signature (1)',
    );
  });

  it('keeps the whole tally, which is what one probe cannot erase', () => {
    // The evidence the old single-reason column destroyed. An investigator's
    // own unsigned probe lands as its own reason beside the forty-one.
    const phrase = refusalReasonsPhrase(verifying());
    expect(phrase).toContain('(41)');
    expect(phrase).toContain('(1)');
  });

  it('says nothing a switched-off headline already said', () => {
    expect(refusalReasonsPhrase(refusal())).toBeNull();
  });

  it('prints a reason this build does not know rather than dropping it', () => {
    // An engine newer than the page still has something true to say, and a
    // silently shorter tally would understate the loss.
    const phrase = refusalReasonsPhrase(verifying({ reasons: { 'from-the-future': 3 } }));
    expect(phrase).toBe('from-the-future (3)');
  });

  it('is null for a tally that did not survive its round trip', () => {
    expect(refusalReasonsPhrase(verifying({ reasons: {} }))).toBeNull();
    expect(refusalReasonsPhrase(verifying({ reasons: { token: 0 } }))).toBeNull();
  });
});

describe('webhookRefusalRowLine', () => {
  it('states the loss, not the symptom', () => {
    // The row already names the hook, so the line says what is happening to
    // its deliveries. "Refused" alone reads as one bad request.
    expect(webhookRefusalRowLine(refusal())).toBe(
      '42 deliveries thrown away, because this webhook is switched off, over 18 days',
    );
    expect(webhookRefusalRowLine(verifying())).toBe(
      '42 deliveries refused, because none of them verified, over 1 day',
    );
    expect(webhookRefusalRowLine(unsigned())).toBe(
      '3 deliveries refused, because none of them carried a signature, over 2 hours',
    );
  });

  it('agrees with the bar about the cause', () => {
    // Two surfaces, one fault. A row saying "switched off" under a bar saying
    // "check the secret" would send the reader two ways at once.
    const row = webhookRefusalRowLine(refusal());
    expect(row).toContain('switched off');
    expect(webhookRefusalNotice(refusal()).title).toContain('switched off');
  });
});
