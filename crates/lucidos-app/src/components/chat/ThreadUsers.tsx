import { useEffect, useState } from 'preact/hooks';
import { changeThreadRole, readThreadUsers, type UserRef, type ThreadUsers, type ThreadRoleChange } from '../../api/client';
import { useThreadLoadable } from '../../hooks/useThreadLoadable';
import { openSettingsSubview } from '../../store/actions/menu';
import { mentionsEnabled, myUserId, usersPanelOpenFor, shownThreadUsers, threadUsersVersion, toggleUsersPanel } from '../../store/actions/users';
import { showToast, threadMap } from '../../store/store';
import { errorDetail } from '../../utils/errorDetail';
import { Disclosure } from '../shared/Disclosure';
import { ChevronDownIcon, UserIcon } from '../shared/icons';

/** A user's initials, as their round mark draws them. A bracketed gateway
 *  label after the name is no part of it. */
export function initialsOf(name: string): string {
  const words = name.replace(/\s*\([^)]*\)\s*$/, '').split(/\s+/).filter(Boolean);
  return words.slice(0, 2).map((w) => w.charAt(0).toUpperCase()).join('');
}

interface Mark {
  user: UserRef;
  role: 'drives' | 'watches';
}

/** The marks in order: the driver, then each watcher not also driving. */
export function usersMarks(users: ThreadUsers): Mark[] {
  const marks: Mark[] = users.driver ? [{ user: users.driver, role: 'drives' }] : [];
  for (const watcher of users.watchers) {
    if (watcher.id !== users.driver?.id) marks.push({ user: watcher, role: 'watches' });
  }
  return marks;
}

/** The open thread's users icon (ADR 0649), at the right end of the title
 *  row on both layouts: the driver's round mark and a chevron, or a users
 *  glyph while nobody drives. It opens the panel `ThreadUsersPanel` draws.
 *  Nothing draws until the roles are read, and never on a personal gateway. */
export function ThreadUsersButton({ threadId }: { threadId: string }) {
  const shown = shownThreadUsers.value;
  if (!shown || shown.threadId !== threadId) return null;
  const open = usersPanelOpenFor.value === threadId;
  const { driver } = shown.users;
  return (
    <button
      type="button"
      class={`thread-users-button${open ? ' open' : ''}`}
      data-role="thread-users-button"
      aria-expanded={open}
      aria-label="Users in this thread"
      onClick={() => toggleUsersPanel(threadId)}
    >
      {driver
        ? <span class="thread-users-mark">{initialsOf(driver.name)}</span>
        : <span class="thread-users-glyph"><UserIcon /></span>}
      <span class="thread-users-chevron"><ChevronDownIcon size="1rem" /></span>
    </button>
  );
}

/** The panel the title row's icon rolls open (ADR 0649): a row per user
 *  with their role, this user's own Watch and Take, and a way to edit
 *  the team. It also owns the read of the thread's users and shares it with
 *  the icon. A team gateway only, so a personal one looks exactly as before. */
export function ThreadUsersPanel({ threadId }: { threadId: string }) {
  const active = mentionsEnabled.value && threadMap.value.get(threadId)?.meta.state === 'active';
  const [users, land] = useThreadLoadable(active ? threadId : null, threadUsersVersion.value, readThreadUsers);
  const [busy, setBusy] = useState(false);
  const me = myUserId.value;
  const loaded = users.status === 'loaded' ? users.data : null;

  useEffect(() => {
    shownThreadUsers.value = loaded ? { threadId, users: loaded } : null;
    return () => { shownThreadUsers.value = null; };
  }, [loaded, threadId]);

  const change = async (what: ThreadRoleChange) => {
    setBusy(true);
    try {
      land(await changeThreadRole(threadId, what));
    } catch (error) {
      showToast(`Could not ${what} this thread: ${errorDetail(error)}`, 'error');
    } finally {
      setBusy(false);
    }
  };

  const drives = !!me && loaded?.driver?.id === me;
  const watches = !!me && !!loaded?.watchers.some((w) => w.id === me);
  const marks = loaded ? usersMarks(loaded) : [];
  const youListed = marks.some(({ user }) => user.id === me);
  return (
    <>
      <Disclosure open={loaded !== null && usersPanelOpenFor.value === threadId}>
        <div class="thread-users-panel" data-role="thread-users-panel" aria-label="Users in this thread">
          {marks.map(({ user, role }) => {
            const you = user.id === me;
            return (
              <div key={user.id} class="thread-users-user">
                <span class={`thread-users-mark${you ? ' thread-users-mark-you' : ''}`}>{initialsOf(user.name)}</span>
                <span class="thread-users-name">{you ? 'You' : user.name}</span>
                <span class="thread-users-role">{role === 'drives' ? 'driving' : 'watching'}</span>
              </div>
            );
          })}
          {me && !youListed && (
            <div class="thread-users-user">
              <span class="thread-users-mark thread-users-mark-you">You</span>
              <span class="thread-users-name">You</span>
            </div>
          )}
          <div class="thread-users-actions">
            {me && (
              <>
                <button type="button" class="action-btn action-btn-secondary" disabled={busy} onClick={() => void change(watches ? 'unwatch' : 'watch')}>
                  {watches ? 'Unwatch' : 'Watch'}
                </button>
                <button type="button" class="action-btn action-btn-secondary" disabled={busy} onClick={() => void change(drives ? 'release' : 'take')}>
                  {drives ? 'Release' : 'Take'}
                </button>
              </>
            )}
            <button type="button" class="accent-link thread-users-edit" onClick={() => openSettingsSubview('users')}>
              Edit users
            </button>
          </div>
        </div>
      </Disclosure>
      <Disclosure open={users.status === 'failed'}>
        <div class="thread-users-panel thread-users-panel-failed" role="alert">
          {users.status === 'failed' && `Could not read the users in this thread: ${users.error}`}
        </div>
      </Disclosure>
    </>
  );
}
