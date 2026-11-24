export function isTransportError(error: unknown): boolean {
  return error instanceof TypeError && /Load failed|Failed to fetch|NetworkError/i.test(error.message);
}

const exhaustedReads = new WeakSet<object>();

export function transportRetryExhausted(error: unknown): boolean {
  return typeof error === 'object' && error !== null && exhaustedReads.has(error);
}

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
      if (typeof retryError === 'object' && retryError !== null) exhaustedReads.add(retryError);
      throw retryError;
    }
  }
}
