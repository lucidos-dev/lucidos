
import { useEffect } from 'preact/hooks';
import { GatewaySwitcher } from '../picker/GatewaySwitcher';
import { Disclosure } from '../shared/Disclosure';
import { revokePairedDevice, type PairedDevice } from '../../api/client/control';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import {
  isManageRefusal,
  loadRemoteGatewayDevices,
  type ManageRefusal,
  loadRemoteGatewayUsers,
  reloadRemoteGatewayLists,
  remoteGatewayDevices,
} from '../../store/actions/remoteGatewayManage';
import { pairedRows } from '../../store/actions/pairedDevices';
import {
  loadPairedRemoteGateways,
  pairedRemoteGateways,
  remoteGatewaysPaired,
  selectSettingsGateway,
  settingsGateway,
} from '../../store/actions/settingsGateway';
import { showConfirm, showToast } from '../../store/store';
import { REMOTE_GATEWAY, WORKSPACE_ID } from '../../utils/basePath';
import { errorDetail } from '../../utils/errorDetail';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkBlock, SkText } from '../shared/Skeleton';
import { deviceDetails, revokePaired, usePairedDevices } from './pairedDevices';
import { DeviceUserPicker } from './UsersSection';

export function remoteSectionTitle(gateway: string | null, localTitle: string): string {
  if (gateway !== null) return `On ${gateway}`;
  return REMOTE_GATEWAY === null && !remoteGatewaysPaired.value ? localTitle : 'On this gateway';
}

export function ManageNotGranted({ gateway }: { gateway: string }) {
  return (
    <div class="settings-section-desc" data-role="manage-not-granted">
      {gateway} does not let devices on this gateway manage its users and devices. To allow
      it, open Settings → Access on {gateway} itself, and turn on the switch on this
      gateway's row under Pair a gateway.
    </div>
  );
}

export function ManageRefused({ gateway, refusal }: { gateway: string; refusal: ManageRefusal }) {
  if (refusal.refused === 'not-granted') return <ManageNotGranted gateway={gateway} />;
  return (
    <div class="settings-section-desc" data-role="remote-gateway-too-old">
      {gateway} runs an older Lucidos that cannot be managed from another gateway. Update
      {' '}{gateway} to manage it from here.
    </div>
  );
}

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

function DeviceListRow({
  device,
  picker,
  onRevoke,
}: {
  device?: PairedDevice;
  picker?: preact.ComponentChildren;
  onRevoke?: () => void;
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
        {device && picker}
        <SkBlock w="4.5rem" h="2rem" round>
          {device && (
            <button class="action-btn action-btn-danger" onClick={onRevoke}>
              Revoke
            </button>
          )}
        </SkBlock>
      </div>
    </div>
  );
}

export function RemoteGatewayDevicesSection({ gateway }: { gateway: string }) {
  const list = remoteGatewayDevices.value;
  const showLoading = useDelayedLoading(list);
  const data = list.status === 'loaded' ? list.data : null;
  return (
    <div class="settings-section">
      <div class="settings-section-title">{remoteSectionTitle(gateway, 'Devices')}</div>
      {isManageRefusal(data) ? <ManageRefused gateway={gateway} refusal={data} /> : (
        <div class="list-rows">
          {list.status === 'failed' && <LoadableError noun={`devices on ${gateway}`} error={list.error} />}
          <LoadingFade
            showSkeleton={showLoading}
            skeleton={<ListSkeletonOf row={() => <DeviceListRow />} count={2} />}
          >
            {data !== null && (data.length === 0
              ? <div class="settings-section-desc">No device is paired with {gateway} itself.</div>
              : data.map((device) => (
                <DeviceListRow
                  key={device.id}
                  device={device}
                  picker={<DeviceUserPicker deviceId={device.id} gateway={gateway} />}
                  onRevoke={() => void revokeRemote(gateway, device, data.length)}
                />
              )))}
          </LoadingFade>
        </div>
      )}
    </div>
  );
}

export function useRemoteGatewayLists(shown: boolean): void {
  const gateway = settingsGateway.value;
  useEffect(() => {
    if (shown) void reloadRemoteGatewayLists();
  }, [shown, gateway]);
}

export function SettingsGatewaySwitcher() {
  const servedByGateway = WORKSPACE_ID !== null;
  useEffect(() => { if (servedByGateway) void loadPairedRemoteGateways(); }, [servedByGateway]);
  usePanelRefresh('settings-gateway-switcher', servedByGateway ? loadPairedRemoteGateways : null);
  const list = pairedRemoteGateways.value;
  const picked = settingsGateway.value;
  const names = list.status === 'loaded' ? list.data.map((remote) => remote.name) : picked === null ? [] : [picked];
  return (
    <Disclosure open={names.length > 0 || list.status === 'failed'}>
      <div class="settings-section" data-role="settings-gateway-switcher">
        {list.status === 'failed' && <LoadableError noun="remote gateways" error={list.error} />}
        <GatewaySwitcher
          views={names.map((gateway) => ({ gateway }))}
          selected={settingsGateway.value}
          disabled={false}
          onSelect={selectSettingsGateway}
        />
      </div>
    </Disclosure>
  );
}

export function RemoteGatewayPageNotice({ gateway, page }: { gateway: string; page: string }) {
  return (
    <div class="settings-section">
      <div class="settings-section-title">On {gateway}</div>
      <div class="settings-section-desc" data-role="remote-gateway-page-notice">
        {gateway}'s {page} page does not open from another gateway yet. Open Settings → {page} on
        {' '}{gateway} itself.
      </div>
    </div>
  );
}

export function ThisGatewayDevicesSection() {
  const { paired: list, selfId, reload } = usePairedDevices();
  const showLoading = useDelayedLoading(list);
  const data = pairedRows(list);
  return (
    <div class="settings-section">
      <div class="settings-section-title">On this gateway</div>
      <div class="list-rows">
        {list.status === 'failed' && <LoadableError noun="devices on this gateway" error={list.error} />}
        <LoadingFade
          showSkeleton={showLoading}
          skeleton={<ListSkeletonOf row={() => <DeviceListRow />} count={2} />}
        >
          {data !== null && (data.length === 0
            ? <div class="settings-section-desc">No device is paired with this gateway.</div>
            : data.map((device) => (
              <DeviceListRow
                key={device.id}
                device={device}
                picker={<DeviceUserPicker deviceId={device.id} />}
                onRevoke={() => void revokePaired(device, { name: device.label, selfId, count: data.length, reload })}
              />
            )))}
        </LoadingFade>
      </div>
    </div>
  );
}
