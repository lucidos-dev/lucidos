
import type { WebhookIngressFamily, WebhookIngressOutage } from '../api/client';
import { formatDurationPhrase } from './formatTime';

export const FAMILY_LABEL: Record<WebhookIngressFamily, string> = { ipv4: 'IPv4', ipv6: 'IPv6' };

export const UNVERIFIED_INGRESS_NOTICE = 'Public webhook ingress is unverified';

const PROBE_INTERVAL = 'every 15 minutes';

export function ingressFamiliesPhrase(families: WebhookIngressFamily[]): string {
  const named = (['ipv4', 'ipv6'] as const)
    .filter((family) => families.includes(family))
    .map((family) => FAMILY_LABEL[family]);
  return named.length > 0 ? named.join(' and ') : 'the public path';
}

function origin(outage: WebhookIngressOutage): string {
  return `${outage.host}:${outage.port}`;
}

function outageAge(outage: WebhookIngressOutage): string {
  return formatDurationPhrase(outage.down_secs);
}

export function webhookIngressNotice(
  outage: WebhookIngressOutage,
): { title: string; detail: string } {
  const families = ingressFamiliesPhrase(outage.families);
  return {
    title: `Webhook deliveries over ${families} are not arriving`,
    detail:
      `${origin(outage)} has not answered over ${families} for ${outageAge(outage)}. ` +
      'A delivery sent that way gets no reply and never reaches this workspace. ' +
      `Rechecked ${PROBE_INTERVAL}.`,
  };
}

export function webhookIngressRowLine(outage: WebhookIngressOutage): string {
  return `Not reachable over ${ingressFamiliesPhrase(outage.families)} for ${outageAge(outage)}`;
}
