import { readDeviceId } from '../../utils/deviceIdHeader';

export function isInAudience(audience: readonly string[] | null | undefined): boolean {
  if (audience == null) return true;
  const id = readDeviceId();
  return id !== null && audience.includes(id);
}
