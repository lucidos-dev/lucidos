import { request } from './_fetch';

export interface UserRef {
  id: string;
  name: string;
}

export interface ThreadUsers {
  root_thread_id: string;
  driver: UserRef | null;
  watchers: UserRef[];
}

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

export const users = {
  list(): Promise<ThreadUsersList> {
    return request<ThreadUsersList>('/threads/users');
  },

  get(threadId: string): Promise<ThreadUsers> {
    return request<ThreadUsers>(userPath(threadId));
  },

  take(threadId: string): Promise<ThreadUsers> {
    return change(threadId, 'take');
  },

  release(threadId: string): Promise<ThreadUsers> {
    return change(threadId, 'release');
  },

  watch(threadId: string): Promise<ThreadUsers> {
    return change(threadId, 'watch');
  },

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
