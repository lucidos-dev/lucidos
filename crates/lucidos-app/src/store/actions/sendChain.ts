import { pageOwnerId } from '../pageOwner';

/** One send's place in its thread's chain. `turn` resolves when the send may
 *  POST; `settled` when it is done, accepted or not. Neither ever rejects, so
 *  one failed send cannot poison the chain for the rest of the thread. */
interface ChainLink {
  turn: Promise<void>;
  settled: Promise<void>;
  /** Consecutive epochs proven by successful ordinary sends in this chain. */
  acceptedEpochs?: { base: number; after: number };
}

/** Per-thread tail of the send chain. It keeps one POST per thread in flight,
 *  so a quick second press does not overtake the first.
 *
 *  The engine orders: each send carries this page's input chain
 *  and an input sequence. The engine refuses a send that a later one from this
 *  page overtook (ADR 0449). Without the pacing, the first of two quick sends
 *  could reach the engine second and be refused.
 *
 *  Keyed per thread: two different threads are independent conversations and
 *  must not queue behind each other. */
const sendChains = new Map<string, ChainLink>();

/** The last input sequence this page claimed per thread. Never reset: a
 *  sequence must keep rising for as long as this page's chain lives. */
const lastInputSeq = new Map<string, number>();

/** Where an input sits in this page's run of inputs to one thread. Sent as
 *  is on every send path, so the engine reads one shape. */
export interface InputPosition {
  input_chain: string;
  input_seq: number;
}

function claimInputPosition(threadId: string): InputPosition {
  const input_seq = (lastInputSeq.get(threadId) ?? 0) + 1;
  lastInputSeq.set(threadId, input_seq);
  return { input_chain: pageOwnerId, input_seq };
}

/** This send's place in its thread's chain: what to await before POSTing, the
 *  release that lets the next send go, and its input position. */
export interface SendSlot {
  /** `null` when nothing was ahead of this send. The caller must then skip the
   *  await entirely rather than await an already-resolved promise: awaiting one
   *  still defers a microtask, which would push the POST out of the caller's
   *  synchronous turn. A lone send dispatches `submitChat` synchronously, and
   *  callers observe that: the compose suite checks the fetch mock right after
   *  `sendFollowup`, without awaiting. Serializing sends must not change when a
   *  lone send goes out. */
  waitForTurn: Promise<void> | null;
  /** Idempotent. Every caller releases in a `finally`. */
  release: () => void;
  position: InputPosition;
  readonly intentEpoch?: number;
  /** Frozen intent epoch, rebased only through a confirmed predecessor. */
  epochForPost: () => number | undefined;
  accepted: (epoch: number | undefined) => void;
}

/** Claim the thread's next chain slot and input sequence. **Synchronous, and
 *  that is the point:** both must follow the order `sendMessage` is CALLED,
 *  not the order each call happens to reach its POST.
 *
 *  `sendMessage` can await before it gets there (`getWebviewContent()` on the
 *  Tauri panel path), and two of those awaits resolve in whatever order the
 *  webview answers. Claiming at POST time would number the second send first.
 *  Claiming here makes the order independent of whatever awaits get added above.
 *
 *  A successor waits for its predecessor to settle, however long that takes.
 *  Every await a send makes has its own deadline, so every send settles. */
export function enterSendChain(threadId: string, intentEpoch?: number): SendSlot {
  const predecessor = sendChains.get(threadId);
  let release!: () => void;
  const settled = new Promise<void>((resolve) => {
    release = resolve;
  });
  const waitForTurn = predecessor
    ? predecessor.turn.then(() => predecessor.settled)
    : null;
  const link: ChainLink = { turn: waitForTurn ?? Promise.resolve(), settled };
  sendChains.set(threadId, link);
  // Drop the entry once the chain drains, so the map doesn't grow one
  // permanent promise per thread the user has ever sent to. Guarded on
  // identity: a send that chained behind this one owns the slot now. A send
  // that ended before its turn still waits for that turn here, so the next
  // send queues behind the earlier sends still retrying.
  void Promise.all([link.turn, settled]).then(() => {
    if (sendChains.get(threadId) === link) sendChains.delete(threadId);
  });
  let submittedEpoch: number | undefined;
  let causalBase = intentEpoch;
  let prepared = false;
  const epochForPost = (): number | undefined => {
    if (!prepared) {
      prepared = true;
      submittedEpoch = intentEpoch;
      const proof = predecessor?.acceptedEpochs;
      if (intentEpoch !== undefined && proof && intentEpoch >= proof.base && intentEpoch <= proof.after) {
        submittedEpoch = proof.after;
        causalBase = proof.base;
      }
    }
    return submittedEpoch;
  };
  return {
    waitForTurn, release, position: claimInputPosition(threadId), intentEpoch, epochForPost,
    accepted: (epoch) => {
      if (epoch === undefined) return;
      const before = epochForPost();
      if (before === undefined) {
        // An accepted send moves the epoch by exactly one, so an unfenced
        // send consumed the draft at `epoch - 1`.
        link.acceptedEpochs = { base: epoch - 1, after: epoch };
      } else if (causalBase !== undefined && epoch === before + 1) {
        link.acceptedEpochs = { base: causalBase, after: epoch };
      }
    },
  };
}
