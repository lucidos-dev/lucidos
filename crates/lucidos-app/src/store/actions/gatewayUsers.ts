
import { computed, signal } from '@preact/signals';
import {
  GatewayError,
  listGatewayUsers,
  type GatewayUser,
  type GatewayUsersResponse,
  type RelayedUser,
} from '../../api/client/control';
import type { Loadable } from '../types';
import { setLoadingIfFresh, toFailed } from '../types';
import { NO_GATEWAY } from './pairedDevices';
import { getDeviceId } from './devices';

export type GatewayUsersLoadable = Loadable<GatewayUsersResponse | typeof NO_GATEWAY>;

export const gatewayUsers = signal<GatewayUsersLoadable>({ status: 'not-loaded' });

export function gatewayUserRows(list: GatewayUsersLoadable): GatewayUser[] | null {
  if (list.status !== 'loaded' || list.data === NO_GATEWAY) return null;
  return list.data.users;
}

export const isTeamGateway = computed(() => (gatewayUserRows(gatewayUsers.value)?.length ?? 0) > 1);

export const UNKNOWN_USER = 'Unknown user';

const nameKey = (name: string) => name.trim().toLowerCase();

function localOfRelayed(list: GatewayUsersResponse, relayed: RelayedUser): GatewayUser | undefined {
  const emails = new Set(
    list.relayed
      .filter((r) => r.user_id === relayed.user_id)
      .flatMap((r) => r.emails.map((e) => e.trim().toLowerCase())),
  );
  return list.users.find((o) => o.emails.some((e) => emails.has(e.toLowerCase())));
}

function relayedName(list: GatewayUsersResponse, relayed: RelayedUser): string {
  const name = relayed.name.trim();
  const own = nameKey(name);
  const shared =
    list.users.some((o) => nameKey(o.name) === own) ||
    list.relayed.some(
      (r) => r.user_id !== relayed.user_id && !localOfRelayed(list, r) && nameKey(r.name) === own,
    );
  return shared ? `${name} (${relayed.via})` : name;
}

function holder(list: GatewayUsersResponse, deviceId: string): { key: string; name: string } | null {
  const local = list.users.find((o) => o.device_ids.includes(deviceId));
  if (local) return { key: `local:${local.id}`, name: local.name };
  const relayed = list.relayed.find((r) => r.device_id === deviceId);
  if (!relayed) return null;
  const merged = localOfRelayed(list, relayed);
  if (merged) return { key: `local:${merged.id}`, name: merged.name };
  return { key: `via:${relayed.via}:${relayed.user_id}`, name: relayedName(list, relayed) };
}

export function gatewayUserName(list: GatewayUsersLoadable, deviceId: string): string | null {
  if (list.status !== 'loaded' || list.data === NO_GATEWAY) return null;
  return holder(list.data, deviceId)?.name ?? null;
}

function userToTellApart(list: GatewayUsersLoadable): GatewayUsersResponse | null {
  if (list.status !== 'loaded' || list.data === NO_GATEWAY) return null;
  const users = list.data;
  return users.users.length < 2 && users.relayed.length === 0 ? null : users;
}

export function userName(list: GatewayUsersLoadable, deviceId: string): string | null {
  const users = userToTellApart(list);
  if (!users) return null;
  return holder(users, deviceId)?.name ?? UNKNOWN_USER;
}

export function senderName(list: GatewayUsersLoadable, readerDeviceId: string, senderDeviceId: string): string | null {
  const users = userToTellApart(list);
  if (!users) return null;
  const reader = holder(users, readerDeviceId);
  if (!reader) return null;
  const sender = holder(users, senderDeviceId);
  if (!sender) return UNKNOWN_USER;
  return sender.key === reader.key ? null : sender.name;
}

export function teammateName(senderDeviceId: string): string | null {
  return senderName(gatewayUsers.value, getDeviceId(), senderDeviceId);
}

export async function loadGatewayUsers(): Promise<void> {
  setLoadingIfFresh(gatewayUsers);
  try {
    gatewayUsers.value = { status: 'loaded', data: await listGatewayUsers() };
  } catch (e) {
    gatewayUsers.value =
      e instanceof GatewayError && e.isAbsent ? { status: 'loaded', data: NO_GATEWAY } : toFailed(e);
  }
}
