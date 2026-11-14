/**
 * The remote gateway's half of Settings → Devices and Users, drawn inside a
 * remote workspace (`/<slug>@<name>/`). It reads and changes that gateway's
 * lists through this gateway's manage hop (ADR 0656).
 *
 * The remote gateway grants the hop per gateway pairing, so a refusal says how
 * to turn it on. A remote gateway too old for the hop hides the section.
 */

import { useEffect } from 'preact/hooks';
import { revokePairedDevice, type PairedDevice } from '../../api/client/control';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import {
  isManageRefusal,
  loadRemoteGatewayDevices,
  loadRemoteGatewayUsers,
  reloadRemoteGatewayLists,
  remoteGatewayDevices,
} from '../../store/actions/remoteGatewayManage';
import { showConfirm, showToast } from '../../store/store';
import { REMOTE_GATEWAY } from '../../utils/basePath';
import { errorDetail } from '../../utils/errorDetail';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkBlock, SkText } from '../shared/Skeleton';
import { deviceDetails } from './pairedDevices';
import { DeviceUserPicker } from './UsersSection';

/** A section's title: "On <name>" for a remote gateway, "On this gateway"
 *  beside one, and `localTitle` where no remote gateway is in view. */
export function remoteSectionTitle(gateway: string | null, localTitle: string): string {
  if (gateway !== null) return `On ${gateway}`;
  return REMOTE_GATEWAY === null ? localTitle : 'On this gateway';
}

/** What a remote gateway that has not granted the hop shows instead. */
export function ManageNotGranted({ gateway }: { gateway: string }) {
  return (
    <div class="settings-section-desc" data-role="manage-not-granted">
      {gateway} does not let devices on this gateway manage its users and devices. To allow
      it, open Settings → Access on {gateway} itself, and turn on the switch on this
      gateway's row under Pair a gateway.
    </div>
  );
}

/** The revoke confirm for a device of a remote gateway. It never signs this
 *  browser out: the device belongs to the other gateway. */
export function remoteRevokeConfirmMessage(device: { label: string; gateway: string; isLast: boolean }): string {
  const { label, gateway } = device;
  if (device.isLast) {
    return (
      `${label} is the only device paired with ${gateway}. Revoking it leaves ${gateway} `
      + 'with no device of its own.\n\n'
      + "Devices on this gateway still open its workspaces through the gateway pairing."
    );
  }
  return `${gateway} will stop answering ${label}. It can pair with ${gateway} again whenever you like.`;
}

async function revokeRemote(gateway: string, device: PairedDevice, count: number): Promise<void> {
  const confirmed = await showConfirm(
    remoteRevokeConfirmMessage({ label: device.label, gateway, isLast: count === 1 }),
    'Revoke',
    { title: `Revoke this device on ${gateway}`, variant: 'danger' },
  );
  if (!confirmed) return;
  try {
    await revokePairedDevice(device.id, gateway);
    showToast(`${device.label} can no longer reach ${gateway}`, 'success');
  } catch (e) {
    showToast(`Couldn't revoke ${device.label} on ${gateway}: ${errorDetail(e)}`, 'error');
  }
  await Promise.all([
    loadRemoteGatewayDevices(gateway),
    loadRemoteGatewayUsers(gateway),
  ]);
}

/** One of the remote gateway's devices. With no device, inside a skeleton,
 *  it is the placeholder. */
function RemoteDeviceRow({
  gateway,
  device,
  count = 0,
}: {
  gateway: string;
  device?: PairedDevice;
  count?: number;
}) {
  return (
    <div class="list-row">
      <div class="list-row-info">
        <SkText class="title" as="div" w="9rem">{device?.label}</SkText>
        <SkText class="list-row-details list-row-details-prose" as="div" w="12rem">
          {device && deviceDetails(device, Date.now())}
        </SkText>
      </div>
      <div class="list-row-actions">
        {device && <DeviceUserPicker deviceId={device.id} gateway={gateway} />}
        <SkBlock w="4.5rem" h="2rem" round>
          {device && (
            <button class="action-btn action-btn-danger" onClick={() => void revokeRemote(gateway, device, count)}>
              Revoke
            </button>
          )}
        </SkBlock>
      </div>
    </div>
  );
}

/** The remote gateway's paired devices, with Revoke and, on a team gateway,
 *  which user holds each. */
export function RemoteGatewayDevicesSection({ gateway }: { gateway: string }) {
  const list = remoteGatewayDevices.value;
  const showLoading = useDelayedLoading(list);
  const data = list.status === 'loaded' ? list.data : null;
  if (isManageRefusal(data) && data.refused === 'too-old') return null;
  return (
    <div class="settings-section">
      <div class="settings-section-title">{remoteSectionTitle(gateway, 'Devices')}</div>
      {isManageRefusal(data) ? <ManageNotGranted gateway={gateway} /> : (
        <div class="list-rows">
          {list.status === 'failed' && <LoadableError noun={`devices on ${gateway}`} error={list.error} />}
          <LoadingFade
            showSkeleton={showLoading}
            skeleton={<ListSkeletonOf row={() => <RemoteDeviceRow gateway={gateway} />} count={2} />}
          >
            {data !== null && (data.length === 0
              ? <div class="settings-section-desc">No device is paired with {gateway} itself.</div>
              : data.map((device) => (
                <RemoteDeviceRow key={device.id} gateway={gateway} device={device} count={data.length} />
              )))}
          </LoadingFade>
        </div>
      )}
    </div>
  );
}

/** Load the remote gateway's lists each time a page that shows them opens. */
export function useRemoteGatewayLists(shown: boolean): void {
  useEffect(() => {
    if (shown) void reloadRemoteGatewayLists();
  }, [shown]);
}
