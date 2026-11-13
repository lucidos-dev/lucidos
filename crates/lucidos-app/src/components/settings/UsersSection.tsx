import type { GatewayUser, GatewayUsersResponse } from '../../api/client/control';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { Dropdown } from '../shared/Dropdown';
import { Explainer } from '../shared/Explainer';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { CloseIcon, EditIcon, TrashIcon } from '../shared/icons';
import { openSettingsSubview } from '../../store/actions/menu';
import { ListSkeletonOf, SkBlock, SkText } from '../shared/Skeleton';
import {
  isTeamGateway,
  gatewayUserRows,
  gatewayUsers,
} from '../../store/actions/gatewayUsers';
import {
  addEmailPrompt, addGatewayUserPrompt, moveDevice, removeEmail,
  removeGatewayUserConfirmed, renameGatewayUserPrompt,
} from '../../store/actions/gatewayUserManagement';
import { NO_GATEWAY } from '../../store/actions/pairedDevices';
import {
  isManageRefusal,
  remoteGatewayUsers,
  type ManageRefusal,
} from '../../store/actions/remoteGatewayManage';
import type { Loadable } from '../../store/types';
import { REMOTE_GATEWAY } from '../../utils/basePath';
import { ManageNotGranted, remoteSectionTitle } from './remoteGatewaySections';

/** The users of `gateway` (`null` for this one), or `null` while there are
 *  none to pick between. This gateway's list counts only on a team gateway. */
function pickableUsers(gateway: string | null): GatewayUser[] | null {
  if (gateway === null) return isTeamGateway.value ? gatewayUserRows(gatewayUsers.value) : null;
  const list = remoteGatewayUsers.value;
  if (list.status !== 'loaded' || isManageRefusal(list.data) || list.data.users.length < 2) return null;
  return list.data.users;
}

/** Which user a paired device belongs to, as a dropdown on its Devices row.
 *  Drawn only with several users: with one there is nothing to pick. */
export function DeviceUserPicker({ deviceId, gateway = null }: { deviceId: string; gateway?: string | null }) {
  const users = pickableUsers(gateway);
  const holder = users?.find((o) => o.device_ids.includes(deviceId));
  if (!users || !holder) return null;
  return (
    <Dropdown
      options={users.map((o) => ({ value: o.id, label: o.name }))}
      value={holder.id}
      onChange={(value) => void moveDevice(gateway, deviceId, value)}
    />
  );
}

/** One user's row. With no user, inside a skeleton, it is the placeholder. */
function UserRow({ gateway, user }: { gateway: string | null; user?: GatewayUser }) {
  const count = user?.device_ids.length ?? 0;
  return (
    <div class="list-row user-row">
      <div class="list-row-info">
        <div class="user-name">
          <SkText class="title" w="8rem">{user?.name}</SkText>
          {user && (
            <button
              type="button"
              class="icon-btn inline-icon"
              aria-label={`Rename ${user.name}`}
              data-tooltip="Rename"
              onClick={() => void renameGatewayUserPrompt(gateway, user)}
            ><EditIcon /></button>
          )}
        </div>
        <div class="list-row-details">
          <SkText w="4rem">{user && (count === 1 ? '1 device' : `${count} devices`)}</SkText>
        </div>
        {user && (
          <div class="list-row-details user-emails">
            {user.emails.map((email) => (
              <span class="user-email" key={email}>
                <span class="user-email-address">{email}</span>
                <button
                  type="button"
                  class="icon-btn inline-icon"
                  aria-label={`Remove ${email}`}
                  data-tooltip="Remove email"
                  onClick={() => void removeEmail(gateway, user, email)}
                ><CloseIcon /></button>
              </span>
            ))}
          </div>
        )}
        <SkBlock w="5.5rem" h="2rem" round>
          {user && <button type="button" class="action-btn" onClick={() => void addEmailPrompt(gateway, user)}>Add email</button>}
        </SkBlock>
      </div>
      <div class="list-row-actions">
        <SkBlock w="var(--icon-tap-target)" h="var(--icon-tap-target)" round>
          {user && (
            <button
              type="button"
              class="icon-btn row-icon"
              aria-label={`Remove ${user.name}`}
              data-tooltip="Revoke every device of this user"
              onClick={() => void removeGatewayUserConfirmed(gateway, user)}
            ><TrashIcon /></button>
          )}
        </SkBlock>
      </div>
    </div>
  );
}

/** Users: the users of a gateway (ADR 0623). A user is a name, the emails
 *  they write from, and the devices they hold. Nobody gets more or less access
 *  by being a user here.
 *
 *  `gateway` names a remote gateway managed through the manage hop (ADR 0656),
 *  or is `null` for this gateway. */
export function UsersSection({ gateway }: { gateway: string | null }) {
  const list: Loadable<GatewayUsersResponse | typeof NO_GATEWAY | ManageRefusal> =
    gateway === null ? gatewayUsers.value : remoteGatewayUsers.value;
  const showLoading = useDelayedLoading(list);
  const data = list.status === 'loaded' ? list.data : null;
  if (data === NO_GATEWAY) {
    return <div class="empty-state">Users live on the workspace gateway, and this engine runs without one.</div>;
  }
  if (isManageRefusal(data) && data.refused === 'too-old') return null;
  const title = remoteSectionTitle(gateway, 'Users');
  const refused = isManageRefusal(data);
  const loaded = data !== null && !isManageRefusal(data) ? data : null;
  // This gateway lists its users only on a team gateway, so a personal one
  // looks as before. A remote gateway is listed whenever it answers.
  const rows = gateway === null ? (isTeamGateway.value ? loaded?.users : null) : loaded?.users;
  return (
    <div class="settings-section users-section">
      <div class="settings-section-title">
        {title}
        <Explainer title="Users">
          <p>
            A user is a name, the emails they use elsewhere, and the devices they
            hold. On a shared gateway it decides whose questions and notifications
            are whose, and who changed what. It never limits what a device can do.
          </p>
          <p>
            Users belong to a machine, not one workspace, so every workspace on a
            machine sees the same users.
          </p>
        </Explainer>
      </div>
      {refused && gateway !== null ? <ManageNotGranted gateway={gateway} /> : (
        <div class="list-rows">
          {list.status === 'failed' && <LoadableError noun={gateway === null ? 'users' : `users on ${gateway}`} error={list.error} />}
          <LoadingFade
            showSkeleton={gateway !== null && showLoading}
            skeleton={<ListSkeletonOf row={() => <UserRow gateway={gateway} />} count={2} />}
          >
            {rows?.map((user) => <UserRow key={user.id} gateway={gateway} user={user} />)}
          </LoadingFade>
          {(gateway === null || loaded) && (
            <div class="list-row">
              <div class="list-row-info">
                <div class="list-row-details list-row-details-prose">
                  {gateway !== null
                    ? `Add someone on ${gateway}, then pair their devices there.`
                    : isTeamGateway.value
                      ? <>Add someone, then pair their devices for them under <button
                          type="button"
                          class="accent-link"
                          onClick={() => openSettingsSubview('access')}
                        >Settings → Access</button>.</>
                      : 'Sharing this machine with a team? Add each user, so work and notifications can be theirs.'}
                </div>
              </div>
              <div class="list-row-actions">
                <button class="action-btn" onClick={() => void addGatewayUserPrompt(gateway)}>Add a user</button>
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/** The Users page: in a remote workspace, the remote gateway's users first,
 *  then this gateway's. Elsewhere, this gateway's alone, as before. */
export function UsersPage() {
  return (
    <>
      {REMOTE_GATEWAY !== null && <UsersSection gateway={REMOTE_GATEWAY} />}
      <UsersSection gateway={null} />
    </>
  );
}
