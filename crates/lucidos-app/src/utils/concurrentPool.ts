export async function runWithConcurrency<T>(
  items: readonly T[],
  limit: number,
  run: (item: T) => Promise<unknown>,
): Promise<void> {
  if (limit <= 0) {
    throw new Error(`runWithConcurrency: limit must be > 0, got ${limit}`);
  }
  if (items.length === 0) return;
  let next = 0;
  const worker = async (): Promise<void> => {
    for (;;) {
      const index = next++;
      if (index >= items.length) return;
      try {
        await run(items[index]);
      } catch (err) {
        console.warn('[concurrentPool] task threw; the pool swallowed it and continued', err);
      }
    }
  };
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker));
}
