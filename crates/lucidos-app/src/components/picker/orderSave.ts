
import { errorDetail } from '../../utils/errorDetail';

export interface OrderSaveDeps {
  draw(order: readonly string[]): void;
  save(order: readonly string[]): Promise<void>;
  report(message: string): void;
}

export interface OrderSaver {
  reorder(next: readonly string[]): Promise<void>;
  readStarted(): number;
  stored(order: readonly string[], read: number): void;
}

export function makeOrderSaver(deps: OrderSaveDeps): OrderSaver {
  let confirmed: readonly string[] = [];
  let waiting: readonly string[] | null = null;
  let running: Promise<void> | null = null;
  let generation = 0;

  async function drain(): Promise<void> {
    while (waiting) {
      const next = waiting;
      waiting = null;
      try {
        await deps.save(next);
        confirmed = next;
      } catch (e) {
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
