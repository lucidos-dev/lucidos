import { API, json, mutatingFetch, throwIfNotOk } from './_core';

export interface UserListing {
  id: string;
  name: string;
  has_device: boolean;
}

export interface UsersResponse {
  users: UserListing[];
  you: string | null;
  team: boolean;
}

export async function listUsers(): Promise<UsersResponse> {
  return json(`${API}/users`);
}

export interface UserRef {
  id: string;
  name: string;
}

export interface ThreadUsers {
  root_thread_id: string;
  driver: UserRef | null;
  watchers: UserRef[];
}

export type ThreadRoleChange = 'take' | 'release' | 'watch' | 'unwatch';

export async function readThreadUsers(threadId: string): Promise<ThreadUsers> {
  return json(`${API}/threads/${encodeURIComponent(threadId)}/users`);
}

export async function changeThreadRole(threadId: string, change: ThreadRoleChange): Promise<ThreadUsers> {
  const res = await mutatingFetch(`${API}/threads/${encodeURIComponent(threadId)}/users/${change}`, { method: 'POST' });
  await throwIfNotOk(res);
  return await res.json() as ThreadUsers;
}

export async function readAgentPause(threadId: string): Promise<boolean> {
  const body = await json<{ paused: boolean }>(`${API}/threads/${encodeURIComponent(threadId)}/agent-pause`);
  return body.paused;
}
