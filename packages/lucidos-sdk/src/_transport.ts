/** Browser transport failures say nothing about the HTTP verdict. */
export function isTransportError(error: unknown): boolean {
  return error instanceof TypeError && /Load failed|Failed to fetch|NetworkError/i.test(error.message);
}

const exhaustedReads = new WeakSet<object>();

/** Whether a read already spent its transport retry. */
export function transportRetryExhausted(error: unknown): boolean {
  return typeof error === 'object' && error !== null && exhaustedReads.has(error);
}

/** Repeat a GET once. The caller owns deadlines and generation checks. */
export async function retryTransportRead<T>(
  request: () => Promise<T>,
  init?: RequestInit,
  checkGeneration: () => void = () => {},
): Promise<T> {
  checkGeneration();
  try {
    return await request();
  } catch (error) {
    if ((init?.method ?? 'GET').toUpperCase() !== 'GET' || init?.signal?.aborted || !isTransportError(error)) throw error;
    checkGeneration();
    try {
      return await request();
    } catch (retryError) {
      // Any failed retry spends the read's budget, a timeout included.
      if (typeof retryError === 'object' && retryError !== null) exhaustedReads.add(retryError);
      throw retryError;
    }
  }
}
