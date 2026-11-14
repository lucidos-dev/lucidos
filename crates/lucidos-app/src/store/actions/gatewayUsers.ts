/**
 * Users: a user, as the gateway knows them (ADR 0623).
 *
 * A user is identity without authority. It decides attribution, routing
 * and read state, and it never allows or refuses anything. The gateway keeps
 * them in its event log. This reads `/~/api/v1/auth/users` like the
 * paired-device list beside it, and answers nothing where no gateway serves
 * the page.
 */

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

/** The users, or `null` when no list is loaded. */
export function gatewayUserRows(list: GatewayUsersLoadable): GatewayUser[] | null {
  if (list.status !== 'loaded' || list.data === NO_GATEWAY) return null;
  return list.data.users;
}

/** More than one user pairs to this gateway. Team controls draw only then,
 *  so a personal gateway looks exactly as before. */
export const isTeamGateway = computed(() => (gatewayUserRows(gatewayUsers.value)?.length ?? 0) > 1);

/** What a message draws for a sender that no user holds. */
export const UNKNOWN_USER = 'Unknown user';

const nameKey = (name: string) => name.trim().toLowerCase();

/** The local user a relayed user is: the one holding an email the other
 *  gateway signed for any of that user's devices. Mirrors
 *  `local_of_relayed` in the engine's `gateway_users.rs`. */
function localOfRelayed(list: GatewayUsersResponse, relayed: RelayedUser): GatewayUser | undefined {
  const emails = new Set(
    list.relayed
      .filter((r) => r.user_id === relayed.user_id)
      .flatMap((r) => r.emails.map((e) => e.trim().toLowerCase())),
  );
  return list.users.find((o) => o.emails.some((e) => emails.has(e.toLowerCase())));
}

/** How a relayed user reads: the local name once merged by email, else
 *  their own name. The gateway label joins it, "Sam (A)", only when another
 *  distinct user holds the same name. `gateway_user_names.json` pins
 *  this rule against the engine copy. */
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

/** Who holds a device. A local user is known by id, and so is a relayed
 *  user merged into one by email. Any other relayed user carries the
 *  gateway it came through, so two gateways never merge two users. */
function holder(list: GatewayUsersResponse, deviceId: string): { key: string; name: string } | null {
  const local = list.users.find((o) => o.device_ids.includes(deviceId));
  if (local) return { key: `local:${local.id}`, name: local.name };
  const relayed = list.relayed.find((r) => r.device_id === deviceId);
  if (!relayed) return null;
  const merged = localOfRelayed(list, relayed);
  if (merged) return { key: `local:${merged.id}`, name: merged.name };
  return { key: `via:${relayed.via}:${relayed.user_id}`, name: relayedName(list, relayed) };
}

/** The holder's name, including on a gateway with one user. */
export function gatewayUserName(list: GatewayUsersLoadable, deviceId: string): string | null {
  if (list.status !== 'loaded' || list.data === NO_GATEWAY) return null;
  return holder(list.data, deviceId)?.name ?? null;
}

/** The loaded list when it has users to tell apart: more than one user,
 *  or any relayed device. `null` for a list that is not loaded (never invent
 *  a teammate) and for a gateway with nobody to tell apart. */
function userToTellApart(list: GatewayUsersLoadable): GatewayUsersResponse | null {
  if (list.status !== 'loaded' || list.data === NO_GATEWAY) return null;
  const users = list.data;
  return users.users.length < 2 && users.relayed.length === 0 ? null : users;
}

/** The user behind a device, for any device including the reader's own.
 *  `null` where there is nobody to tell apart, "Unknown user" for a device
 *  no user holds. */
export function userName(list: GatewayUsersLoadable, deviceId: string): string | null {
  const users = userToTellApart(list);
  if (!users) return null;
  return holder(users, deviceId)?.name ?? UNKNOWN_USER;
}

/**
 * The name to draw for a device's message, or `null` when it reads as the
 * reader's own: the same user, one gateway with nobody to tell apart, or a
 * list that is not loaded. A device that no user holds on a team gateway
 * is "Unknown user".
 */
export function senderName(list: GatewayUsersLoadable, readerDeviceId: string, senderDeviceId: string): string | null {
  const users = userToTellApart(list);
  if (!users) return null;
  const reader = holder(users, readerDeviceId);
  if (!reader) return null;
  const sender = holder(users, senderDeviceId);
  if (!sender) return UNKNOWN_USER;
  return sender.key === reader.key ? null : sender.name;
}

/** `senderName` for this reader, reactive through the `users` signal. */
export function teammateName(senderDeviceId: string): string | null {
  return senderName(gatewayUsers.value, getDeviceId(), senderDeviceId);
}

/** Fetch the list. A refetch keeps the loaded list on screen until it lands. */
export async function loadGatewayUsers(): Promise<void> {
  setLoadingIfFresh(gatewayUsers);
  try {
    gatewayUsers.value = { status: 'loaded', data: await listGatewayUsers() };
  } catch (e) {
    gatewayUsers.value =
      e instanceof GatewayError && e.isAbsent ? { status: 'loaded', data: NO_GATEWAY } : toFailed(e);
  }
}
