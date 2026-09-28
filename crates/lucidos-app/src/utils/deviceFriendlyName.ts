/**
 * What to call a device: the one rule every screen names a device by.
 *
 * The name someone typed on the Devices row wins. Then the pairing label, the
 * name the device got when it paired ("Safari on iPhone"). Only with neither is
 * it `device-` and the first eight characters of its id, never the whole id: a
 * 36-character uuid is unreadable, and as a row heading it wraps.
 *
 * The engine's `friendly_device_name` applies the same order, so the agent and
 * every screen agree. Never build a device name anywhere else;
 * `__tests__/device-names-go-through-one-function.test.ts` holds that line.
 */

export interface DeviceNameSources {
  id: string;
  /** The typed name, from the engine's device row. */
  name?: string | null;
  /** The pairing label: the gateway's when it answered, else the engine's copy. */
  pairingLabel?: string | null;
}

export function deviceFriendlyName({ id, name, pairingLabel }: DeviceNameSources): string {
  const chosen = [name, pairingLabel].find((n) => n != null && n.trim() !== '');
  return chosen ?? `device-${id.slice(0, 8)}`;
}

/**
 * What to call a device, from the two rows it can have: the engine's per
 * workspace, and the gateway's pairing. Every screen names a device through
 * this, so the Devices page and a message's Origin cannot disagree.
 *
 * The gateway's label is preferred to the engine's copy of it: it is the live
 * one, and the only one a device with no engine row has. `||`, not `??`, so an
 * empty gateway label does not hide the engine's copy.
 */
export function deviceName(
  id: string,
  device?: { name: string | null; pairing_label: string | null },
  paired?: { label: string },
): string {
  return deviceFriendlyName({
    id,
    name: device?.name,
    pairingLabel: paired?.label || device?.pairing_label,
  });
}
