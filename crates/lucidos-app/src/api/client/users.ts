import { API, json, mutatingFetch, throwIfNotOk } from './_core';

/** One user the gateway knows. See engine `GET /api/v1/users`. */
export interface UserListing {
  id: string;
  /** "Sam", or "Sam (A)" for a relayed user another user shares a name with. */
  name: string;
  /** False for a user known only by email. No push reaches them. */
  has_device: boolean;
}

export interface UsersResponse {
  users: UserListing[];
  /** This device's user, or `null` when no user holds it. */
  you: string | null;
  /** Two or more users. Only then does a send take mentions (ADR 0649). */
  team: boolean;
}

export async function listUsers(): Promise<UsersResponse> {
  return json(`${API}/users`);
}

/** A user as a thread role names them. */
export interface UserRef {
  id: string;
  name: string;
}

/** A root thread's driver and watchers. See `GET /api/v1/threads/:id/users`. */
export interface ThreadUsers {
  root_thread_id: string;
  driver: UserRef | null;
  watchers: UserRef[];
}

export type ThreadRoleChange = 'take' | 'release' | 'watch' | 'unwatch';

export async function readThreadUsers(threadId: string): Promise<ThreadUsers> {
  return json(`${API}/threads/${encodeURIComponent(threadId)}/users`);
}

/** Take, Release, Watch or Unwatch a thread's root as this device's user.
 *  Answers the roles after the change. */
export async function changeThreadRole(threadId: string, change: ThreadRoleChange): Promise<ThreadUsers> {
  const res = await mutatingFetch(`${API}/threads/${encodeURIComponent(threadId)}/users/${change}`, { method: 'POST' });
  await throwIfNotOk(res);
  return await res.json() as ThreadUsers;
}

export async function readAgentPause(threadId: string): Promise<boolean> {
  const body = await json<{ paused: boolean }>(`${API}/threads/${encodeURIComponent(threadId)}/agent-pause`);
  return body.paused;
}
