import { readerDevices } from '../store';
import { setLoadingIfFresh, toFailed } from '../types';
import { readerDevices as fetchReaderDevices } from '../../api/client';
import { loadSlice, type SliceOf } from '../bootstrapSliceLoads';

export const readerDevicesSlice: SliceOf<'reader_devices'> = {
  begin: () => setLoadingIfFresh(readerDevices),
  apply: (data) => { readerDevices.value = { status: 'loaded', data }; },
  fail: (error) => { readerDevices.value = toFailed(error); },
};

export function loadReaderDevices(): Promise<void> {
  return loadSlice('reader_devices', readerDevicesSlice, () => fetchReaderDevices());
}

export function readIsMine(actor: unknown): boolean {
  const reader = readerDevices.value;
  const mine = reader.status === 'loaded' ? reader.data.devices : null;
  if (mine === null) return true;
  const origin = actor as { kind?: unknown; device_id?: unknown } | null | undefined;
  return origin?.kind === 'device' && typeof origin.device_id === 'string' && mine.includes(origin.device_id);
}
