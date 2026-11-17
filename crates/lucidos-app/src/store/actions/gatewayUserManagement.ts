/** User-management actions, loaded with the Users settings surface. */
import {
  addGatewayUser, addGatewayUserEmail, moveDeviceToGatewayUser,
  removeGatewayUser, removeGatewayUserEmail, renameGatewayUser,
  type GatewayUser, type GatewayUsersResponse,
} from '../../api/client/control';
import { showConfirm, showPrompt, showToast } from '../store';
import { gatewayUsers } from './gatewayUsers';
import { loadPairedDevices } from './pairedDevices';
import { loadRemoteGatewayDevices, remoteGatewayUsers } from './remoteGatewayManage';

/** Run one change on `gateway` (`null` for this one). Every route answers the
 *  whole list, which lands at once in that gateway's signal. */
async function change(
  gateway: string | null,
  what: string,
  call: () => Promise<GatewayUsersResponse>,
): Promise<boolean> {
  try {
    const data = await call();
    if (gateway === null) gatewayUsers.value = { status: 'loaded', data };
    else remoteGatewayUsers.value = { status: 'loaded', data };
    return true;
  } catch (e) {
    showToast(`Could not ${what}: ${e instanceof Error ? e.message : String(e)}`, 'error');
    return false;
  }
}

export async function addGatewayUserPrompt(gateway: string | null): Promise<void> {
  const name = await showPrompt('Who is joining?', {
    title: gateway === null ? 'Add a user' : `Add a user on ${gateway}`,
    placeholder: 'Name',
    okLabel: 'Add',
  });
  if (name === null || !name.trim()) return;
  await change(gateway, `add ${name.trim()}`, () => addGatewayUser(gateway, name));
}

export async function renameGatewayUserPrompt(gateway: string | null, user: GatewayUser): Promise<void> {
  const name = await showPrompt(`Rename ${user.name}`, {
    title: 'Rename user',
    defaultValue: user.name,
    okLabel: 'Rename',
  });
  if (name === null || !name.trim() || name.trim() === user.name) return;
  await change(gateway, `rename ${user.name}`, () => renameGatewayUser(gateway, user.id, name));
}

export async function addEmailPrompt(gateway: string | null, user: GatewayUser): Promise<void> {
  const email = await showPrompt(
    `The address ${user.name} uses in your issue tracker or chat tool. Lucidos matches it to know who wrote.`,
    { title: 'Add an email', placeholder: 'name@example.com', okLabel: 'Add' },
  );
  if (email === null || !email.trim()) return;
  await change(gateway, `add an email to ${user.name}`, () => addGatewayUserEmail(gateway, user.id, email));
}

export async function removeEmail(gateway: string | null, user: GatewayUser, email: string): Promise<void> {
  await change(gateway, `remove ${email}`, () => removeGatewayUserEmail(gateway, user.id, email));
}

/** Remove a user after a confirm that says it revokes their devices. */
export async function removeGatewayUserConfirmed(gateway: string | null, user: GatewayUser): Promise<void> {
  const count = user.device_ids.length;
  const devices = count === 1 ? 'their one paired device' : `all ${count} of their paired devices`;
  const where = gateway === null ? '' : ` on ${gateway}`;
  const confirmed = await showConfirm(
    count === 0
      ? `${user.name} holds no paired device${where}. Lucidos forgets them.`
      : `This revokes ${devices}${where}, then forgets ${user.name}. That is how a team offboards someone.`,
    'Remove',
    { title: `Remove ${user.name}?`, variant: 'danger' },
  );
  if (!confirmed) return;
  if (await change(gateway, `remove ${user.name}`, () => removeGatewayUser(gateway, user.id))) {
    await (gateway === null ? loadPairedDevices() : loadRemoteGatewayDevices(gateway));
  }
}

export async function moveDevice(gateway: string | null, deviceId: string, userId: string): Promise<void> {
  await change(gateway, 'move the device', () => moveDeviceToGatewayUser(gateway, deviceId, userId));
}
