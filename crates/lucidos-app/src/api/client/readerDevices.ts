import { API, json } from './_core';

/** See engine `GET /api/v1/operators/me`. */
export interface ReaderDevices {
  /** The devices whose reads count as this one's: its person's, on a team
   *  gateway. `null` reads as the whole team. */
  devices: string[] | null;
}

export async function readerDevices(): Promise<ReaderDevices> {
  return json(`${API}/operators/me`);
}
