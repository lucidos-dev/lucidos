/**
 * Slots for the engine calls an app frame makes through the shell.
 *
 * Those calls share the shell's HTTP/1.1 pool to the engine origin, about six
 * connections. With no limit, one busy app fills the pool, and its own clicks,
 * every shell request and every other frame wait behind it. So at most
 * `BRIDGED_FETCH_SLOTS` run at once, and free slots go round robin across
 * frames, to the frame with the fewest calls running.
 */

/** The pool is about six and the shell's event stream holds one. Three bridged
 *  calls leave the shell two connections of its own. */
export const BRIDGED_FETCH_SLOTS = 3;

interface Queued {
  queuedAt: number;
  deadline: number;
  start: (remainingMs: number) => void;
  reject: (error: Error) => void;
}

/** What the lanes are doing now, for a diagnostic. Ages are in milliseconds. */
export interface LanesSnapshot {
  running: number;
  queued: number;
  oldestRunningMs: number | null;
  oldestQueuedMs: number | null;
}

/** The frame a queue belongs to. A disconnected one has gone. */
export interface Lane {
  readonly isConnected: boolean;
}

export class FetchLanes<L extends Lane> {
  /** Insertion order breaks a tie between lanes. A lane leaves when it empties
   *  and rejoins at the back when it has more to send. */
  private readonly queues = new Map<L, Queued[]>();
  /** Calls running, per lane. A lane with none has no entry. */
  private readonly running = new Map<L, number>();
  private inFlight = 0;
  /** When each running call began. */
  private readonly runStarts = new Set<{ startedAt: number }>();

  constructor(private readonly slots: number) {}

  /**
   * Run `send` in the lane's turn, with the time left before `deadline`.
   *
   * A request whose deadline passed while it waited is rejected unsent: the app
   * has given up on it, and a write it was told failed must not land. A
   * request from a frame that unmounted is dropped the same way.
   */
  enqueue<T>(lane: L, deadline: number, send: (remainingMs: number) => Promise<T>): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const queued: Queued = {
        queuedAt: Date.now(),
        deadline,
        reject,
        start: (remainingMs) => {
          const run = { startedAt: Date.now() };
          this.began(lane, run);
          send(remainingMs).then(resolve, reject).finally(() => {
            this.ended(lane, run);
            this.pump();
          });
        },
      };
      const queue = this.queues.get(lane);
      if (queue) queue.push(queued);
      else this.queues.set(lane, [queued]);
      this.pump();
    });
  }

  private pump(): void {
    while (this.inFlight < this.slots) {
      const next = this.takeNext();
      if (!next) return;
      const { lane, queued } = next;
      const remainingMs = queued.deadline - Date.now();
      if (!lane.isConnected) queued.reject(new Error('The app frame is gone, so its call was not sent'));
      else if (remainingMs <= 0) queued.reject(new Error('The app stopped waiting before the call could be sent'));
      else queued.start(remainingMs);
    }
  }

  /** Read-only: the diagnostic reads it without moving a call. */
  snapshot(now = Date.now()): LanesSnapshot {
    const oldest = (times: Iterable<number>): number | null => {
      const first = Math.min(...times);
      return Number.isFinite(first) ? now - first : null;
    };
    const waiting = [...this.queues.values()].flat();
    return {
      running: this.inFlight,
      queued: waiting.length,
      oldestRunningMs: oldest([...this.runStarts].map((r) => r.startedAt)),
      oldestQueuedMs: oldest(waiting.map((q) => q.queuedAt)),
    };
  }

  private began(lane: L, run: { startedAt: number }): void {
    this.runStarts.add(run);
    this.inFlight++;
    this.running.set(lane, (this.running.get(lane) ?? 0) + 1);
  }

  private ended(lane: L, run: { startedAt: number }): void {
    this.runStarts.delete(run);
    this.inFlight--;
    const left = (this.running.get(lane) ?? 1) - 1;
    if (left > 0) this.running.set(lane, left);
    else this.running.delete(lane);
  }

  /** The oldest request of the waiting lane with the fewest calls running. */
  private takeNext(): { lane: L; queued: Queued } | null {
    let lane: L | null = null;
    let fewest = Infinity;
    for (const waiting of this.queues.keys()) {
      const count = this.running.get(waiting) ?? 0;
      if (count < fewest) {
        lane = waiting;
        fewest = count;
      }
    }
    if (lane === null) return null;
    const queue = this.queues.get(lane) as Queued[];
    const queued = queue.shift() as Queued;
    this.queues.delete(lane);
    if (queue.length > 0) this.queues.set(lane, queue);
    return { lane, queued };
  }
}
