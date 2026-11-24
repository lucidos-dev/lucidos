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
