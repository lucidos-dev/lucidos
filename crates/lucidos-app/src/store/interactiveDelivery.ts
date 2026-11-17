/** Shared claims, rather than matching BroadcastChannel snapshots, authorize
 * interactive presentation. The exclusive lock is never held for user input. */
import { chooseWindow, PEER_CHANGE_GRACE_MS, type PeerState } from './windowPeers';
import type {
  DeliveryJournal, DeliveryParticipant, InteractiveDeliveryBackend, InteractiveRequest,
} from './interactiveDeliveryRecords';

export type DeliveryView = Omit<DeliveryParticipant, 'ownerId' | 'revision'>;
export interface InteractiveDeliveryDependencies {
  ownerId: string;
  backend: InteractiveDeliveryBackend;
  exclusive<T>(body: () => Promise<T>): Promise<T>;
  liveOwners(): Promise<ReadonlySet<string> | null>;
  readSelf(): DeliveryView;
  /** Synchronous acceptance, not completion of an answer or external operation. */
  present(request: InteractiveRequest): boolean;
  changed(): void;
  uncertain(request: InteractiveRequest): void;
  now(): number;
}

const MAX_RECEIPTS = 512;

function preferences(request: InteractiveRequest): Array<(state: PeerState) => boolean> {
  const preference = request.preference;
  if ('threadId' in preference) return [(state) => state.focusedThreadId === preference.threadId, () => true];
  if ('fromApp' in preference) return [(state) => state.appId !== null, () => true];
  return [
    (state) => state.authorizing !== null && (state.authorizing.provider === null
      || state.authorizing.provider === preference.provider),
    (state) => state.authorizing !== null,
    () => true,
  ];
}

/** Current visibility filters before previous-content grace, as in windowPeers. */
export function nominatedParticipant(journal: DeliveryJournal, request: InteractiveRequest, now: number): string | null {
  const record = journal.requests[request.id];
  const participants = Object.values(journal.participants);
  const shown = participants.filter((member) => member.state.nativeShown !== false);
  const eligible = (shown.length ? shown : participants).filter((member) => record?.delivery.status !== 'pending'
    || record.delivery.declined[member.ownerId] !== member.revision)
    .sort((a, b) => a.ownerId.localeCompare(b.ownerId));
  for (const qualifies of preferences(request)) {
    const chosen = chooseWindow(eligible.map((member) => member.state), (state) => {
      const member = eligible.find((candidate) => candidate.state === state)!;
      return qualifies(state) || (member.previous !== null && now - member.changedAt < PEER_CHANGE_GRACE_MS
        && qualifies(member.previous));
    });
    if (chosen !== null) return eligible.find((member) => member.state.id === chosen)!.ownerId;
  }
  return null;
}

function trimReceipts(journal: DeliveryJournal): void {
  // An unanswered form is still live even after its first presentation.
  const completed = Object.entries(journal.requests).filter(([, record]) => record.delivery.status === 'resolved'
    || (record.delivery.status === 'presented' && record.request.kind !== 'form'));
  completed.sort((a, b) => ('at' in a[1].delivery ? a[1].delivery.at : 0)
    - ('at' in b[1].delivery ? b[1].delivery.at : 0));
  for (const [id] of completed.slice(0, Math.max(0, completed.length - MAX_RECEIPTS))) delete journal.requests[id];
}

/** Separate instances share only the backend and Web Lock, never a peer map. */
export function createInteractiveDelivery(deps: InteractiveDeliveryDependencies) {
  let stopped = false;
  const noticed = new Set<string>();

  function publishSelf(journal: DeliveryJournal, force = false): void {
    const view = deps.readSelf();
    const old = journal.participants[deps.ownerId];
    if (!old || force || JSON.stringify({ state: old.state, previous: old.previous, changedAt: old.changedAt }) !== JSON.stringify(view)) {
      journal.participants[deps.ownerId] = { ...view, ownerId: deps.ownerId, revision: (old?.revision ?? 0) + 1 };
    }
  }

  async function run(change?: (journal: DeliveryJournal) => void): Promise<void> {
    if (stopped) return;
    const uncertain: InteractiveRequest[] = [];
    let changed = false;
    async function update(change: (journal: DeliveryJournal) => void): Promise<DeliveryJournal> {
      let differs = false;
      const result = await deps.backend.update((journal) => {
        const before = JSON.stringify(journal);
        change(journal);
        differs = before !== JSON.stringify(journal);
      });
      changed ||= differs;
      return result;
    }
    try {
      await deps.exclusive(async () => {
        if (stopped) return;
        const live = await deps.liveOwners();
        if (live === null) throw new Error('Interactive requests need working Web Locks. Reopen Lucidos in a supported browser.');
        let journal = await update((current) => {
          for (const owner of Object.keys(current.participants)) {
            if (!live.has(owner)) delete current.participants[owner];
          }
          // A previous holder released the delivery lock with a claim outstanding.
          // It may have changed the DOM, even if its page is still alive.
          for (const record of Object.values(current.requests)) {
            if (record.delivery.status === 'claimed') record.delivery = { status: 'uncertain' };
          }
          publishSelf(current);
          change?.(current);
          trimReceipts(current);
        });
        for (const record of Object.values(journal.requests)) {
          if (stopped) break;
          if (record.suspended) continue;
          if (record.delivery.status === 'uncertain') {
            if (!noticed.has(record.request.id) && nominatedParticipant(journal, record.request, deps.now()) === deps.ownerId) {
              uncertain.push(record.request);
            }
            continue;
          }
          if (record.delivery.status !== 'pending') continue;
          const declined = record.delivery.declined;
          // A local hide/show may have arrived while the database committed.
          journal = await update((current) => publishSelf(current));
          if (nominatedParticipant(journal, record.request, deps.now()) !== deps.ownerId) continue;
          journal = await update((current) => {
            publishSelf(current);
            if (nominatedParticipant(current, record.request, deps.now()) === deps.ownerId) {
              current.requests[record.request.id].delivery = { status: 'claimed', ownerId: deps.ownerId };
            }
          });
          if (journal.requests[record.request.id].delivery.status !== 'claimed') continue;
          // No await between final local validation and synchronous presentation.
          const member = journal.participants[deps.ownerId];
          const fresh = deps.readSelf();
          if (JSON.stringify(fresh) !== JSON.stringify({ state: member.state, previous: member.previous, changedAt: member.changedAt })) {
            await update((current) => {
              publishSelf(current);
              current.requests[record.request.id].delivery = { status: 'pending', declined: {} };
            });
            continue;
          }
          let accepted: boolean;
          try { accepted = deps.present(record.request); }
          catch (error) {
            await update((current) => { current.requests[record.request.id].delivery = { status: 'uncertain' }; });
            throw error;
          }
          await update((current) => {
            current.requests[record.request.id].delivery = accepted
              ? { status: 'presented', ownerId: deps.ownerId, at: deps.now() }
              : { status: 'pending', declined: { ...declined, [deps.ownerId]: member.revision } };
            trimReceipts(current);
          });
        }
      });
    } finally {
      // Claims whose receipt failed still wake peers for uncertain recovery.
      if (changed) deps.changed();
    }
    for (const request of uncertain) {
      noticed.add(request.id);
      deps.uncertain(request);
    }
  }

  return {
    formIds: () => deps.exclusive(async () => {
      const journal = await deps.backend.update(() => {});
      return Object.values(journal.requests)
        .filter((record) => record.request.kind === 'form' && record.delivery.status !== 'resolved')
        .map((record) => record.request.id.slice('form:'.length));
    }),
    publish: () => run((journal) => publishSelf(journal, true)),
    reconcileForms: (known: string[], open: string[], resolved: string[]) => run((journal) => {
      const pending = new Set(open);
      const completed = new Set(resolved);
      for (const id of known) {
        const record = journal.requests[id];
        if (record?.request.kind !== 'form') continue;
        if (completed.has(id)) record.delivery = { status: 'resolved', at: deps.now() };
        if (record.suspended && pending.has(id) && record.delivery.status === 'pending') {
          record.delivery.declined = {};
        }
        record.suspended = !pending.has(id);
      }
    }),
    reconsider: () => run(),
    submit: (request: InteractiveRequest) => run((journal) => {
      journal.requests[request.id] ??= { request, delivery: { status: 'pending', declined: {} } };
    }),
    resolve: (id: string) => run((journal) => {
      const record = journal.requests[id];
      if (record) record.delivery = { status: 'resolved', at: deps.now() };
      else journal.requests[id] = {
        request: { id, kind: 'form', payload: {}, preference: { provider: null } },
        delivery: { status: 'resolved', at: deps.now() },
      };
    }),
    retry: (id: string) => run((journal) => {
      const record = journal.requests[id];
      if (record?.delivery.status === 'uncertain') record.delivery = { status: 'pending', declined: {} };
      noticed.delete(id);
    }),
    async stop() {
      stopped = true;
      await deps.exclusive(async () => {
        await deps.backend.update((journal) => { delete journal.participants[deps.ownerId]; });
      });
      deps.changed();
    },
  };
}
