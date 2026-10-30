import type { GatewayTls } from '../../api/client/control';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import type { Loadable } from '../../store/types';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { SkText, SkeletonProvider } from '../shared/Skeleton';

/** Pure: the row's title and sentence, from the gateway's TLS state.
 *
 *  Every state says something true. With no HTTPS it always says why, since
 *  plain HTTP with no stated reason is the failure this row exists to stop. */
export function gatewayHttpsSummary(tls: GatewayTls): { title: string; detail: string } {
  if (tls.source === 'operator') {
    return {
      title: 'HTTPS: your own certificate',
      detail: 'LUCIDOS_TLS_CERT names the certificate, so every address serves HTTPS.',
    };
  }
  if (tls.source === 'tailnet' && tls.https_origin !== null) {
    const valid = tls.expires_at ? ` Valid until ${new Date(tls.expires_at).toLocaleDateString()}.` : '';
    const renewal = tls.error
      ? ` The last certificate update hit a problem, and the current certificate still serves: ${tls.error}`
      : ' It renews on its own.';
    return {
      title: 'HTTPS: automatic via your tailnet',
      detail: `Other devices open ${tls.https_origin} with a certificate your tailnet issues.${valid}${renewal}`,
    };
  }
  return {
    title: 'HTTPS: not available',
    detail: `${tls.error ?? 'The gateway gave no reason.'} A device that reaches this gateway over plain HTTP cannot install the app or get push.`,
  };
}

/** The HTTPS row under "The machine running Lucidos". The certificate is the
 *  gateway's, so this reads its control plane, and the caller renders it only
 *  behind the gateway. */
export function GatewayHttpsRow({ tls }: { tls: Loadable<GatewayTls> }) {
  const showLoading = useDelayedLoading(tls);
  if (tls.status === 'failed') return <LoadableError noun="HTTPS state" error={tls.error} />;
  return (
    <LoadingFade
      showSkeleton={showLoading}
      skeleton={<SkeletonProvider><HttpsRow /></SkeletonProvider>}
    >
      {tls.status === 'loaded' && <HttpsRow summary={gatewayHttpsSummary(tls.data)} />}
    </LoadingFade>
  );
}

/** With no `summary`, inside a `SkeletonProvider`, it is the placeholder. */
function HttpsRow({ summary }: { summary?: { title: string; detail: string } }) {
  return (
    <div class="list-rows">
      <div class="list-row">
        <div class="list-row-info">
          <SkText class="title" as="div" w="12rem">{summary?.title}</SkText>
          <SkText class="list-row-details list-row-details-prose" as="div" w="80%">
            {summary?.detail}
          </SkText>
        </div>
      </div>
    </div>
  );
}
