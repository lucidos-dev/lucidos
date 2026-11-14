/**
 * One ask at a time, for an ask whose answer can go stale while it runs.
 *
 * A plain call joins the ask in flight. A `fresh` call needs an answer begun
 * after it. So one more ask runs once the current one ends, and the call
 * waits for that one too.
 */
export function singleFlight(ask: () => Promise<void>): (fresh?: boolean) => Promise<void> {
  let inFlight: Promise<void> | null = null;
  let again = false;
  return (fresh = false) => {
    if (inFlight) {
      if (fresh) again = true;
      return inFlight;
    }
    inFlight = (async () => {
      try {
        do {
          again = false;
          await ask();
        } while (again);
      } finally {
        inFlight = null;
      }
    })();
    return inFlight;
  };
}
