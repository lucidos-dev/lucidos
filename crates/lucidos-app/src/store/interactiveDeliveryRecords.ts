/** Workspace-local journal. Responses and form field values never belong here. */
import { BOUNDED, openDeviceDatabase } from './deviceDatabase';
import type { PeerState } from './windowPeers';

export interface DeliveryParticipant {
  ownerId: string;
  state: PeerState;
  previous: PeerState | null;
  changedAt: number;
  revision: number;
}

export interface InteractiveRequest {
  id: string;
  kind: 'form' | 'navigation' | 'push-enable' | 'oauth-connected';
  payload: Record<string, unknown>;
  preference: { threadId: string } | { fromApp: true } | { provider: string | null };
}

type DeliveryState =
  | { status: 'pending'; declined: Record<string, number> }
  | { status: 'claimed'; ownerId: string }
  | { status: 'presented'; ownerId: string; at: number }
  | { status: 'uncertain' }
  | { status: 'resolved'; at: number };

export interface DeliveryRecord {
  request: InteractiveRequest;
  delivery: DeliveryState;
  suspended?: boolean;
}

export interface DeliveryJournal {
  participants: Record<string, DeliveryParticipant>;
  requests: Record<string, DeliveryRecord>;
}

export const emptyDeliveryJournal = (): DeliveryJournal => ({ participants: {}, requests: {} });

export interface InteractiveDeliveryBackend {
  update(change: (journal: DeliveryJournal) => void): Promise<DeliveryJournal>;
}

/** Callers hold the workspace Web Lock through selection and presentation.
 * Each mutation also commits in one transaction, so a dead page leaves either
 * its old journal or its complete new journal, never half a claim. */
export function createInteractiveDeliveryBackend(name: string): InteractiveDeliveryBackend {
  const database = openDeviceDatabase(name, (db) => db.createObjectStore('journal'));
  return {
    async update(change) {
      let result!: DeliveryJournal;
      await database.transact('journal', 'readwrite', (store) => {
        const get = store.get('current');
        get.onsuccess = () => {
          result = get.result ?? emptyDeliveryJournal();
          change(result);
          store.put(result, 'current');
        };
      }, BOUNDED);
      return result;
    },
  };
}
