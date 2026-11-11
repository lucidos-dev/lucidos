import { useState } from 'preact/hooks';
import { changeThreadRole, readThreadPeople, type PersonRef, type ThreadPeople, type ThreadRoleChange } from '../../api/client';
import { useThreadLoadable } from '../../hooks/useThreadLoadable';
import { mentionsEnabled, myPersonId, threadPeopleVersion } from '../../store/actions/people';
import { showToast, threadMap } from '../../store/store';
import { errorDetail } from '../../utils/errorDetail';
import { Disclosure } from '../shared/Disclosure';

/** A person's initials, as their round mark draws them. */
export function initialsOf(name: string): string {
  const words = name.split(/\s+/).filter((w) => w && w.toLowerCase() !== 'via');
  return words.slice(0, 2).map((w) => w.charAt(0).toUpperCase()).join('');
}

interface Mark {
  person: PersonRef;
  role: 'drives' | 'watches';
}

/** The marks in order: the driver, then each watcher not also driving. */
export function peopleMarks(people: ThreadPeople): Mark[] {
  const marks: Mark[] = people.driver ? [{ person: people.driver, role: 'drives' }] : [];
  for (const watcher of people.watchers) {
    if (watcher.id !== people.driver?.id) marks.push({ person: watcher, role: 'watches' });
  }
  return marks;
}

/** "People in this thread" (ADR 0649): who drives and watches the thread's
 *  root, with this person's own Watch and Take. A team gateway only, so a
 *  personal one looks exactly as before. */
export function ThreadPeopleRow({ threadId }: { threadId: string }) {
  const active = mentionsEnabled.value && threadMap.value.get(threadId)?.meta.state === 'active';
  const [people, land] = useThreadLoadable(active ? threadId : null, threadPeopleVersion.value, readThreadPeople);
  const [busy, setBusy] = useState(false);
  const me = myPersonId.value;

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

  const loaded = people.status === 'loaded' ? people.data : null;
  const drives = !!me && loaded?.driver?.id === me;
  const watches = !!me && !!loaded?.watchers.some((w) => w.id === me);
  return (
    <>
      <Disclosure open={loaded !== null}>
        <div class="thread-people-row" data-role="thread-people-row" aria-label="People in this thread">
          {loaded && peopleMarks(loaded).map(({ person, role }) => {
            const you = person.id === me;
            return (
              <span
                key={person.id}
                class={`thread-people-mark${you ? ' thread-people-mark-you' : ''}`}
                data-tooltip={`${you ? 'You' : person.name} ${you ? (role === 'drives' ? 'drive' : 'watch') : role}`}
              >
                {you ? 'You' : initialsOf(person.name)}
              </span>
            );
          })}
          <span class="thread-people-summary">
            {loaded?.driver ? `${loaded.driver.id === me ? 'You' : loaded.driver.name} ${loaded.driver.id === me ? 'drive' : 'drives'}` : 'Nobody drives'}
          </span>
          {me && (
            <span class="thread-people-actions">
              <button type="button" class="action-btn action-btn-secondary" disabled={busy} onClick={() => void change(watches ? 'unwatch' : 'watch')}>
                {watches ? 'Unwatch' : 'Watch'}
              </button>
              <button type="button" class="action-btn action-btn-secondary" disabled={busy} onClick={() => void change(drives ? 'release' : 'take')}>
                {drives ? 'Release' : 'Take'}
              </button>
            </span>
          )}
        </div>
      </Disclosure>
      <Disclosure open={people.status === 'failed'}>
        <div class="thread-people-row thread-people-row-failed" role="alert">
          {people.status === 'failed' && `Could not read the people in this thread: ${people.error}`}
        </div>
      </Disclosure>
    </>
  );
}
