// From its own module: many suites mock the client barrel whole, and this
// constant is read when the module loads.
import { SUBMIT_CHAT_TIMEOUT_MS } from '../../api/client/chat';
import { SEND_RETRY_DEADLINE_MS } from './sendRetry';

/** The longest a send may hold its thread's chain slot once its turn has come,
 *  before the next send goes out anyway. A send's last attempt starts within
 *  `SEND_RETRY_DEADLINE_MS` and gives up after `SUBMIT_CHAT_TIMEOUT_MS`, so a
 *  send that behaves always settles first. This is the net for one that hangs
 *  some other way. */
export const SEND_CHAIN_MAX_HOLD_MS = SEND_RETRY_DEADLINE_MS + SUBMIT_CHAT_TIMEOUT_MS + 5_000;

/** One send's place in its thread's chain. `turn` resolves when the send may
 *  POST; `settled` when it is done, accepted or not. Neither ever rejects, so
 *  one failed send cannot poison the chain for the rest of the thread. */
interface ChainLink {
  turn: Promise<void>;
  settled: Promise<void>;
}

/** Per-thread tail of the send chain. This is what keeps a device's messages
 *  in the order the user pressed send.
 *
 *  The engine stamps `MessageReceived.created` when it emits the event, and the
 *  request carries no client-side ordering data. So two POSTs in flight at once
 *  can arrive out of order, and the reversal is then unrecoverable. The later
 *  message wins the race and starts the turn. The earlier one is queued and
 *  injected into it as a follow-up. See
 *  `docs/plans/2026-07-30-serialize-chat-sends-per-thread.md`.
 *
 *  Keyed per thread: two different threads are independent conversations and
 *  must not queue behind each other. */
const sendChains = new Map<string, ChainLink>();

/** Resolve when `p` settles, or after `ms`, whichever comes first. `p` never
 *  rejects (see `ChainLink`), so the success arm alone covers both outcomes. */
function settledOrTimedOut(p: Promise<void>, ms: number): Promise<void> {
  return new Promise<void>((resolve) => {
    const timer = setTimeout(resolve, ms);
    void p.then(() => {
      clearTimeout(timer);
      resolve();
    });
  });
}

/** This send's place in its thread's chain: what to await before POSTing, and
 *  the release that lets the next send go. */
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
}

/** Claim the thread's next chain slot. **Synchronous, and that is the point:**
 *  the slot must be taken in the order `sendMessage` is CALLED, not in the
 *  order each call happens to reach its POST.
 *
 *  `sendMessage` can await before it gets there (`getWebviewContent()` on the
 *  Tauri panel path), and two of those awaits resolve in whatever order the
 *  webview answers. Claiming the slot at POST time would let the second send
 *  overtake the first there, with the slots reversed. That is the exact bug
 *  the chain exists to prevent, one layer up. Claiming it
 *  here makes the guarantee independent of whatever awaits get added above.
 *
 *  The safety timer starts when the predecessor's OWN turn comes, never when
 *  this send entered, so every send behind a hung one keeps its place. */
export function enterSendChain(threadId: string): SendSlot {
  const predecessor = sendChains.get(threadId);
  let release!: () => void;
  const settled = new Promise<void>((resolve) => {
    release = resolve;
  });
  const waitForTurn = predecessor
    ? predecessor.turn.then(() => settledOrTimedOut(predecessor.settled, SEND_CHAIN_MAX_HOLD_MS))
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
  return { waitForTurn, release };
}

/** Run a send other than a chat message in the thread's chain, such as an
 *  answer to a question card. A message sent after it then waits for it. */
export async function inSendChain<T>(threadId: string, send: () => Promise<T>): Promise<T> {
  const slot = enterSendChain(threadId);
  try {
    if (slot.waitForTurn) await slot.waitForTurn;
    return await send();
  } finally {
    slot.release();
  }
}
