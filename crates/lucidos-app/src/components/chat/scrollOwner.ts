import { signal } from '@preact/signals';

type StandingOwner = Readonly<{ kind: 'reader' }> | Readonly<{ kind: 'ride' }>;
type LinkWriter = Readonly<{ kind: 'link'; claim: object }>;
export type TemporaryOwner =
  | Readonly<{ kind: 'landing' | 'navigation' | 'restore' | 'hold' }>
  | LinkWriter;

type LinkClaim = Readonly<{ token: object; phase: 'resolving' | 'landed' }>;

type ActiveOwner = StandingOwner | { kind: 'temporary'; writer: TemporaryOwner; resumes: ActiveOwner };
type ScrollAct = {
  type: 'scroll';
  atEdge: boolean;
  tookOver: boolean;
  gesture: boolean;
  placement: boolean;
  live: boolean;
};

type Act =
  | { type: 'arm' | 'resume' | 'park' | 'disarm' }
  | { type: 'claim-link'; claim: object }
  | { type: 'release-link'; claim?: object }
  | { type: 'land-link'; writer: LinkWriter; atEdge: boolean }
  | { type: 'link-landed'; claim: object; atEdge: boolean }
  | { type: 'begin-restore' | 'end-restore'; attempt: object }
  | { type: 'take'; owner: TemporaryOwner }
  | { type: 'finish'; owner: TemporaryOwner }
  | ScrollAct;

export function scrollTakesRide(act: Pick<ScrollAct, 'atEdge' | 'tookOver' | 'gesture' | 'placement'>): boolean {
  return !act.atEdge && act.tookOver && (act.gesture || act.placement);
}

function standingOwner(owner: ActiveOwner): StandingOwner {
  return owner.kind === 'temporary' ? standingOwner(owner.resumes) : owner;
}

function replaceStanding(owner: ActiveOwner, standing: StandingOwner): ActiveOwner {
  if (owner.kind !== 'temporary') return standing;
  return { ...owner, resumes: replaceStanding(owner.resumes, standing) };
}

function finishWriter(owner: ActiveOwner, writer: TemporaryOwner): ActiveOwner {
  if (owner.kind !== 'temporary') return owner;
  if (owner.writer === writer) return owner.resumes;
  const resumes = finishWriter(owner.resumes, writer);
  return resumes === owner.resumes ? owner : { ...owner, resumes };
}

export function createScrollOwner() {
  const state = signal<{ request: 'off' | 'armed'; owner: ActiveOwner }>({
    request: 'off',
    owner: { kind: 'reader' },
  });
  let claim: LinkClaim | null = null;
  let linkLanding: 'at-edge' | 'off-edge' | null = null;
  const pendingRestores = new WeakSet<object>();

  function transition(act: Act): void {
    const { owner, request } = state.peek();
    switch (act.type) {
      case 'arm':
        state.value = { request: 'armed', owner: replaceStanding(owner, { kind: 'ride' }) };
        break;
      case 'resume':
        if (request === 'armed') state.value = { request, owner: replaceStanding(owner, { kind: 'ride' }) };
        break;
      case 'disarm':
        state.value = { request: 'off', owner: replaceStanding(owner, { kind: 'reader' }) };
        break;
      case 'park':
        state.value = { request, owner: replaceStanding(owner, { kind: 'reader' }) };
        break;
      case 'scroll':
        if (standingOwner(owner).kind === 'ride') {
          if (scrollTakesRide(act)) transition({ type: act.live ? 'disarm' : 'park' });
        } else if (act.atEdge && (act.gesture || act.placement)) {
          transition({ type: 'resume' });
        }
        break;
      case 'claim-link':
        claim = { token: act.claim, phase: 'resolving' };
        linkLanding = null;
        break;
      case 'release-link':
        if (act.claim === undefined || claim?.token === act.claim) {
          claim = null;
          linkLanding = null;
        }
        break;
      case 'land-link':
        if (!act.atEdge) transition({ type: 'disarm' });
        else transition({ type: 'resume' });
        transition({ type: 'take', owner: act.writer });
        break;
      case 'link-landed':
        linkLanding = act.atEdge ? 'at-edge' : 'off-edge';
        if (claim?.token === act.claim) claim = { token: act.claim, phase: 'landed' };
        break;
      case 'begin-restore':
        pendingRestores.add(act.attempt);
        break;
      case 'end-restore':
        pendingRestores.delete(act.attempt);
        break;
      case 'take':
        state.value = { request, owner: { kind: 'temporary', writer: act.owner, resumes: owner } };
        break;
      case 'finish': {
        const next = finishWriter(owner, act.owner);
        if (next !== owner) state.value = { request, owner: next };
        break;
      }
    }
  }

  function landLink(claim: object, atEdge: boolean): LinkWriter {
    const writer: LinkWriter = { kind: 'link', claim };
    transition({ type: 'land-link', writer, atEdge });
    return writer;
  }

  function withWriter<T>(writer: TemporaryOwner, fn: () => T): T {
    transition({ type: 'take', owner: writer });
    try {
      return fn();
    } finally {
      transition({ type: 'finish', owner: writer });
    }
  }

  function beginRestore() {
    const attempt = {};
    const writer = { kind: 'restore' } as const;
    transition({ type: 'begin-restore', attempt });
    return {
      waiting: () => pendingRestores.has(attempt),
      finish: () => transition({ type: 'end-restore', attempt }),
      write: (fn: () => void) => withWriter(writer, fn),
    };
  }

  return {
    transition,
    beginRestore,
    withWriter,
    landLink,
    owner: () => {
      const { owner } = state.value;
      return owner.kind === 'temporary' ? owner.writer : owner;
    },
    armed: () => state.value.request === 'armed',
    ridePending: () => standingOwner(state.value.owner).kind === 'ride',
    canRide: () => state.value.owner.kind === 'ride' && claim === null,
    claim: () => claim?.token ?? null,
    claimHasLanded: () => claim?.phase === 'landed',
    linkLandedOffEdge: () => linkLanding === 'off-edge',
  };
}

export const scrollPosition = createScrollOwner();
