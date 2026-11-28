import { API, json } from './_core';

export interface ReaderDevices {
  devices: string[] | null;
}

export async function readerDevices(): Promise<ReaderDevices> {
  return json(`${API}/users/me`);
}
