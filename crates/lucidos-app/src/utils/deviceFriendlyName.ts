import { userAgentDeviceLabel } from './deviceLabel';


export interface DeviceNameSources {
  id: string;
  name?: string | null;
  pairingLabel?: string | null;
  userAgent?: string | null;
}

export function deviceFriendlyName({ id, name, pairingLabel, userAgent }: DeviceNameSources): string {
  const chosen = [name, pairingLabel].find((n) => n != null && n.trim() !== '');
  if (chosen) return chosen;
  const short = id.slice(0, 8);
  const fromUserAgent = userAgentDeviceLabel(userAgent);
  return fromUserAgent ? `${fromUserAgent} (${short})` : `device-${short}`;
}

export function deviceName(
  id: string,
  device?: { name: string | null; pairing_label: string | null; user_agent?: string | null },
  paired?: { label: string },
): string {
  return deviceFriendlyName({
    id,
    name: device?.name,
    pairingLabel: paired?.label || device?.pairing_label,
    userAgent: device?.user_agent,
  });
}
