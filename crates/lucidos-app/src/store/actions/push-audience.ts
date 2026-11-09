import { readDeviceId } from '../../utils/deviceIdHeader';

/** Whether a push with this audience is for the device in hand. The audience
 *  names a work item's driver and watchers (ADR 0623); absent means every
 *  device. A page with no device id yet is in no named audience. */
export function isInAudience(audience: readonly string[] | null | undefined): boolean {
  if (audience == null) return true;
  const id = readDeviceId();
  return id !== null && audience.includes(id);
}
