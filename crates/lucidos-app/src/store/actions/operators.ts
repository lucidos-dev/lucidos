/**
 * Operators: a person, as the gateway knows them (ADR 0623).
 *
 * An operator is identity without authority. It decides attribution, routing
 * and read state, and it never allows or refuses anything. The gateway keeps
 * them in its event log. This reads `/~/api/v1/auth/operators` like the
 * paired-device list beside it, and answers nothing where no gateway serves
 * the page.
 */

import { computed, signal } from '@preact/signals';
import {
  addOperator,
  addOperatorEmail,
  GatewayError,
  listOperators,
  moveDeviceToOperator,
  removeOperator,
  removeOperatorEmail,
  renameOperator,
  type GatewayOperator,
  type OperatorsResponse,
} from '../../api/client/control';
import type { Loadable } from '../types';
import { setLoadingIfFresh, toFailed } from '../types';
import { showConfirm, showPrompt, showToast } from '../store';
import { loadPairedDevices, NO_GATEWAY } from './pairedDevices';
import { getDeviceId } from './devices';

export type OperatorsLoadable = Loadable<OperatorsResponse | typeof NO_GATEWAY>;

export const operators = signal<OperatorsLoadable>({ status: 'not-loaded' });

/** The people, or `null` when no list is loaded. */
export function operatorRows(list: OperatorsLoadable): GatewayOperator[] | null {
  if (list.status !== 'loaded' || list.data === NO_GATEWAY) return null;
  return list.data.operators;
}

/** More than one person pairs to this gateway. Team controls draw only then,
 *  so a personal gateway looks exactly as before. */
export const isTeamGateway = computed(() => (operatorRows(operators.value)?.length ?? 0) > 1);

/** What a message draws for a sender that no operator holds. */
export const UNKNOWN_PERSON = 'Unknown person';

/** Who holds a device. A local person is known by id. A relayed one carries
 *  the gateway it came through, so two gateways never merge two people. */
function holder(list: OperatorsResponse, deviceId: string): { key: string; name: string } | null {
  const local = list.operators.find((o) => o.device_ids.includes(deviceId));
  if (local) return { key: `local:${local.id}`, name: local.name };
  const relayed = list.relayed.find((r) => r.device_id === deviceId);
  if (relayed) {
    return { key: `via:${relayed.via}:${relayed.operator_id}`, name: `${relayed.name} via ${relayed.via}` };
  }
  return null;
}

/** The holder's name, including on a gateway with one person. */
export function operatorName(list: OperatorsLoadable, deviceId: string): string | null {
  if (list.status !== 'loaded' || list.data === NO_GATEWAY) return null;
  return holder(list.data, deviceId)?.name ?? null;
}

/** The loaded list when it has people to tell apart: more than one operator,
 *  or any relayed device. `null` for a list that is not loaded (never invent
 *  a teammate) and for a gateway with nobody to tell apart. */
function peopleToTellApart(list: OperatorsLoadable): OperatorsResponse | null {
  if (list.status !== 'loaded' || list.data === NO_GATEWAY) return null;
  const people = list.data;
  return people.operators.length < 2 && people.relayed.length === 0 ? null : people;
}

/** The person behind a device, for any device including the reader's own.
 *  `null` where there is nobody to tell apart, "Unknown person" for a device
 *  no operator holds. */
export function personName(list: OperatorsLoadable, deviceId: string): string | null {
  const people = peopleToTellApart(list);
  if (!people) return null;
  return holder(people, deviceId)?.name ?? UNKNOWN_PERSON;
}

/**
 * The name to draw for a device's message, or `null` when it reads as the
 * reader's own: the same person, one gateway with nobody to tell apart, or a
 * list that is not loaded. A device that no operator holds on a team gateway
 * is "Unknown person".
 */
export function senderName(list: OperatorsLoadable, readerDeviceId: string, senderDeviceId: string): string | null {
  const people = peopleToTellApart(list);
  if (!people) return null;
  const reader = holder(people, readerDeviceId);
  if (!reader) return null;
  const sender = holder(people, senderDeviceId);
  if (!sender) return UNKNOWN_PERSON;
  return sender.key === reader.key ? null : sender.name;
}

/** `senderName` for this reader, reactive through the `operators` signal. */
export function teammateName(senderDeviceId: string): string | null {
  return senderName(operators.value, getDeviceId(), senderDeviceId);
}

/** Fetch the list. A refetch keeps the loaded list on screen until it lands. */
export async function loadOperators(): Promise<void> {
  setLoadingIfFresh(operators);
  try {
    operators.value = { status: 'loaded', data: await listOperators() };
  } catch (e) {
    operators.value =
      e instanceof GatewayError && e.isAbsent ? { status: 'loaded', data: NO_GATEWAY } : toFailed(e);
  }
}

/** Run one change. Every route answers the whole list, which lands at once. */
async function change(what: string, call: () => Promise<OperatorsResponse>): Promise<boolean> {
  try {
    operators.value = { status: 'loaded', data: await call() };
    return true;
  } catch (e) {
    showToast(`Could not ${what}: ${e instanceof Error ? e.message : String(e)}`, 'error');
    return false;
  }
}

export async function addOperatorPrompt(): Promise<void> {
  const name = await showPrompt('Who is joining?', {
    title: 'Add a person',
    placeholder: 'Name',
    okLabel: 'Add',
  });
  if (name === null || !name.trim()) return;
  await change(`add ${name.trim()}`, () => addOperator(name));
}

export async function renameOperatorPrompt(operator: GatewayOperator): Promise<void> {
  const name = await showPrompt(`Rename ${operator.name}`, {
    title: 'Rename person',
    defaultValue: operator.name,
    okLabel: 'Rename',
  });
  if (name === null || !name.trim() || name.trim() === operator.name) return;
  await change(`rename ${operator.name}`, () => renameOperator(operator.id, name));
}

export async function addEmailPrompt(operator: GatewayOperator): Promise<void> {
  const email = await showPrompt(
    `The address ${operator.name} uses in your issue tracker or chat tool. Lucidos matches it to know who wrote.`,
    { title: 'Add an email', placeholder: 'name@example.com', okLabel: 'Add' },
  );
  if (email === null || !email.trim()) return;
  await change(`add an email to ${operator.name}`, () => addOperatorEmail(operator.id, email));
}

export async function removeEmail(operator: GatewayOperator, email: string): Promise<void> {
  await change(`remove ${email}`, () => removeOperatorEmail(operator.id, email));
}

/** Remove a person after a confirm that says it revokes their devices. */
export async function removeOperatorConfirmed(operator: GatewayOperator): Promise<void> {
  const count = operator.device_ids.length;
  const devices = count === 1 ? 'their one paired device' : `all ${count} of their paired devices`;
  const confirmed = await showConfirm(
    count === 0
      ? `${operator.name} holds no paired device. Lucidos forgets them.`
      : `This revokes ${devices}, then forgets ${operator.name}. That is how a team offboards someone.`,
    'Remove',
    { title: `Remove ${operator.name}?`, variant: 'danger' },
  );
  if (!confirmed) return;
  if (await change(`remove ${operator.name}`, () => removeOperator(operator.id))) {
    await loadPairedDevices();
  }
}

export async function moveDevice(deviceId: string, operatorId: string): Promise<void> {
  await change('move the device', () => moveDeviceToOperator(deviceId, operatorId));
}
