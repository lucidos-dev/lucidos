/**
 * Saving the workspace order from the picker: the list moves at once, and a
 * refused save puts it back where the gateway still has it.
 *
 * Saves run one at a time, so the gateway stores moves in the order the user
 * made them. A move made while a save is in flight waits, and only the newest
 * waiting order is sent.
 */

import { errorDetail } from '../../utils/errorDetail';

export interface OrderSaveDeps {
  /** Draw this order now. */
  draw(order: readonly string[]): void;
  /** Store it on the gateway. */
  save(order: readonly string[]): Promise<void>;
  /** Tell the user why it did not stick. */
  report(message: string): void;
}

export interface OrderSaver {
  /** Draw `next` now and store it. Resolves once no save is left to run. */
  reorder(next: readonly string[]): Promise<void>;
  /** Call before a read of the stored order, and pass the result to `stored`. */
  readStarted(): number;
  /** The order a read found. Drawn unless a save started or ran since the read began. */
  stored(order: readonly string[], read: number): void;
}

export function makeOrderSaver(deps: OrderSaveDeps): OrderSaver {
  // The order the gateway holds, as the last read or save showed it.
  let confirmed: readonly string[] = [];
  let waiting: readonly string[] | null = null;
  let running: Promise<void> | null = null;
  // Moves whenever a save starts or ends, so a read from before cannot land.
  let generation = 0;

  async function drain(): Promise<void> {
    while (waiting) {
      const next = waiting;
      waiting = null;
      try {
        await deps.save(next);
        confirmed = next;
      } catch (e) {
        // A newer order is on its way and carries this move too.
        if (!waiting) {
          deps.draw(confirmed);
          deps.report(`Could not save the workspace order: ${errorDetail(e)}`);
        }
      }
    }
    running = null;
    generation++;
  }

  return {
    reorder(next) {
      deps.draw(next);
      waiting = next;
      generation++;
      running ??= drain();
      return running;
    },
    readStarted: () => generation,
    stored(order, read) {
      if (running || read !== generation) return;
      confirmed = order;
      deps.draw(order);
    },
  };
}
