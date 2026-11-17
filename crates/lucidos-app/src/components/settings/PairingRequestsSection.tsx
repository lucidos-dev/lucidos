/**
 * "Waiting for your Allow" on Settings → Access: pairings a paired device must
 * answer.
 *
 * Every process on the machine can read the local token. So once a device is
 * paired, a code a process mints waits for a paired device (plan invariant 9).
 * The redeemer waits on its own screen, and this device gets a notification
 * that opens this page. The section draws only while a request waits, or when
 * the read failed.
 */

import { useCallback, useEffect, useState } from 'preact/hooks';
import {
  answerPairingRequest,
  listPairingRequests,
  type PairingRequest,
} from '../../api/client/control';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { useVersionedRefresh } from '../../hooks/useVersionedRefresh';
import { pairingRequestsVersion } from '../../store/pairingRequests';
import { showToast } from '../../store/store';
import { loadingIfFresh, toFailed, type Loadable } from '../../store/types';
import { errorDetail } from '../../utils/errorDetail';
import { ArrivalList } from '../shared/ArrivalList';
import { useArrivals } from '../shared/arrivals';
import { Disclosure } from '../shared/Disclosure';
import { LoadableError } from '../shared/LoadableError';

/** Pure: the line under a request's title. */
export function pairingRequestDetail(request: PairingRequest): string {
  const what = request.kind === 'device'
    ? 'A device, with a code made on the machine running Lucidos'
    : 'Another gateway, with a secret made on the machine running Lucidos';
  const minutes = Math.max(1, Math.ceil(request.expires_in_secs / 60));
  return `${what} · ${minutes} min left`;
}

export function PairingRequestsSection({ available }: { available: boolean }) {
  const [requests, setRequests] = useState<Loadable<PairingRequest[]>>({ status: 'not-loaded' });
  const [answering, setAnswering] = useState<string | null>(null);

  const reload = useCallback((): Promise<unknown> => {
    if (!available) return Promise.resolve();
    setRequests(loadingIfFresh);
    return listPairingRequests().then(
      (data) => setRequests({ status: 'loaded', data }),
      (e) => setRequests(toFailed(e)),
    );
  }, [available]);
  useEffect(() => { void reload(); }, [reload]);
  usePanelRefresh('pairing requests', reload);
  useVersionedRefresh(pairingRequestsVersion.value, answering !== null, () => { void reload(); });

  const waiting = requests.status === 'loaded' ? requests.data : [];
  const arrived = useArrivals(requests.status === 'loaded' ? waiting.map((r) => r.id) : null);

  const answer = async (request: PairingRequest, verb: 'allow' | 'decline') => {
    setAnswering(request.id);
    try {
      await answerPairingRequest(request.id, verb);
    } catch (e) {
      showToast(`Couldn't answer "${request.label}": ${errorDetail(e)}`, 'error');
    } finally {
      setAnswering(null);
      await reload();
    }
  };

  // Nothing waits almost always, so the section rolls in only when something
  // does. Loading draws nothing for the same reason: an empty section is the
  // usual answer.
  return (
    <Disclosure open={requests.status === 'failed' || waiting.length > 0}>
      <div class="settings-section">
        {/* No search anchor: the section exists only while a request waits,
            so a search entry would land on nothing almost always. */}
        <div class="settings-section-title">Waiting for your Allow</div>
        {requests.status === 'failed' ? (
          <LoadableError noun="pairing requests" error={requests.error} />
        ) : (
          <div class="list-rows">
            <ArrivalList items={waiting} keyOf={(r) => r.id} arrived={arrived}>
              {(request, markerClass) => (
                <div class={markerClass ? `list-row ${markerClass}` : 'list-row'}>
                  <div class="list-row-info">
                    <div class="title">{request.label} wants to pair</div>
                    <div class="list-row-details list-row-details-prose">
                      {pairingRequestDetail(request)}
                    </div>
                  </div>
                  <div class="list-row-actions">
                    <button
                      class="action-btn action-btn-secondary"
                      disabled={answering !== null}
                      onClick={() => void answer(request, 'decline')}
                    >
                      Don't allow
                    </button>
                    <button
                      class="action-btn action-btn-confirm"
                      disabled={answering !== null}
                      onClick={() => void answer(request, 'allow')}
                    >
                      Allow
                    </button>
                  </div>
                </div>
              )}
            </ArrivalList>
          </div>
        )}
      </div>
    </Disclosure>
  );
}
