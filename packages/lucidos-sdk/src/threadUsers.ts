import { request } from './_fetch';

/** A user, as a role in a thread names them. Someone reached through a
 *  gateway pairing reads "Sam", or "Sam (A)" when another user shares the
 *  name. */
export interface UserRef {
  id: string;
  name: string;
}

/** Who steers a root thread and its sub-threads (ADR 0623). Roles are never
 *  authority: anyone can still answer any gate. */
export interface ThreadUsers {
  root_thread_id: string;
  driver: UserRef | null;
  watchers: UserRef[];
}

/** One open root thread in the factory listing. `needs_team` is the Team
 *  view: an open gate with no driver, or one that escalated. `yours` marks
 *  the ones the calling device's user drives or watches. */
export interface ThreadUsersSummary {
  root_thread_id: string;
  title: string | null;
  last_activity: string;
  driver: UserRef | null;
  watchers: UserRef[];
  gate_open: boolean;
  escalated: boolean;
  needs_team: boolean;
  yours: boolean;
}

/** The open root threads, newest activity first, and the caller's user
 *  (`null` for a device no user holds). */
export interface ThreadUsersList {
  items: ThreadUsersSummary[];
  user: UserRef | null;
}

function userPath(threadId: string): string {
  return `/threads/${encodeURIComponent(threadId)}/users`;
}

function change(threadId: string, verb: string): Promise<ThreadUsers> {
  return request<ThreadUsers>(`${userPath(threadId)}/${verb}`, { method: 'POST' });
}

/** The driver and watchers of a root thread. Any thread id names its root.
 *  Each change acts as the user holding this device, and returns the
 *  users after it. A device that belongs to no user gets a 409. See
 *  system-knowhow/js-sdk.md § lucidos.threads.users. */
export const users = {
  /** The open root threads: sent and not archived. */
  list(): Promise<ThreadUsersList> {
    return request<ThreadUsersList>('/threads/users');
  },

  /** The root thread's driver and watchers. */
  get(threadId: string): Promise<ThreadUsers> {
    return request<ThreadUsers>(userPath(threadId));
  },

  /** Drive the root thread. On one someone else drives, this is Take over. */
  take(threadId: string): Promise<ThreadUsers> {
    return change(threadId, 'take');
  },

  /** Stop driving. Changes nothing when you do not drive it. */
  release(threadId: string): Promise<ThreadUsers> {
    return change(threadId, 'release');
  },

  /** Watch: its gate pushes reach you beside the driver. */
  watch(threadId: string): Promise<ThreadUsers> {
    return change(threadId, 'watch');
  },

  /** Stop watching, from any of your devices. */
  unwatch(threadId: string): Promise<ThreadUsers> {
    return change(threadId, 'unwatch');
  },
};

/** `threads.users` as apps written before the user rename call it: `list()`
 *  also names the caller `person`. See
 *  docs/temporary-measures.md, "SDK threadPeople alias". */
export const people = {
  ...users,
  list(): Promise<ThreadUsersList & { person: UserRef | null }> {
    return users.list().then((list) => ({ ...list, person: list.user }));
  },
};
