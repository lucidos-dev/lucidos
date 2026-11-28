import type { ComponentChildren } from 'preact';
import { settingsGateway } from '../../store/actions/settingsGateway';
import { MobileAccessPage } from './MobileAccessPage';
import { RemoteGatewayPageNotice, SettingsGatewaySwitcher } from './remoteGatewaySections';
import { UpdatesPage } from './UpdatesPage';
import { UsersSection } from './UsersSection';

function ThisGatewayOnly({ page, children }: { page: string; children: ComponentChildren }) {
  const gateway = settingsGateway.value;
  return (
    <>
      <SettingsGatewaySwitcher />
      {gateway === null ? children : <RemoteGatewayPageNotice gateway={gateway} page={page} />}
    </>
  );
}

export function GatewayAccessPage() {
  return <ThisGatewayOnly page="Access"><MobileAccessPage /></ThisGatewayOnly>;
}

export function GatewayUpdatesPage() {
  return <ThisGatewayOnly page="Updates & Installs"><UpdatesPage /></ThisGatewayOnly>;
}

export function GatewayUsersPage() {
  return (
    <>
      <SettingsGatewaySwitcher />
      <UsersSection gateway={settingsGateway.value} />
    </>
  );
}
