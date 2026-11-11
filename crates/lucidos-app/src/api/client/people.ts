import { API, json, mutatingFetch, throwIfNotOk } from './_core';

/** One person the gateway knows. See engine `GET /api/v1/operators/people`. */
export interface Person {
  id: string;
  /** "Sam", or "Sam via A" for a relayed person. */
  name: string;
  /** False for a person known only by email. No push reaches them. */
  has_device: boolean;
}

export interface PeopleResponse {
  people: Person[];
  /** This device's person, or `null` when no person holds it. */
  you: string | null;
  /** Two or more people. Only then does a send take mentions (ADR 0649). */
  team: boolean;
}

export async function listPeople(): Promise<PeopleResponse> {
  return json(`${API}/operators/people`);
}

/** A person as a thread role names them. */
export interface PersonRef {
  id: string;
  name: string;
}

/** A root thread's driver and watchers. See `GET /api/v1/threads/:id/people`. */
export interface ThreadPeople {
  root_thread_id: string;
  driver: PersonRef | null;
  watchers: PersonRef[];
}

export type ThreadRoleChange = 'take' | 'release' | 'watch' | 'unwatch';

export async function readThreadPeople(threadId: string): Promise<ThreadPeople> {
  return json(`${API}/threads/${encodeURIComponent(threadId)}/people`);
}

/** Take, Release, Watch or Unwatch a thread's root as this device's person.
 *  Answers the roles after the change. */
export async function changeThreadRole(threadId: string, change: ThreadRoleChange): Promise<ThreadPeople> {
  const res = await mutatingFetch(`${API}/threads/${encodeURIComponent(threadId)}/people/${change}`, { method: 'POST' });
  await throwIfNotOk(res);
  return await res.json() as ThreadPeople;
}

export async function readAgentPause(threadId: string): Promise<boolean> {
  const body = await json<{ paused: boolean }>(`${API}/threads/${encodeURIComponent(threadId)}/agent-pause`);
  return body.paused;
}

/** Flip the *agent pause* by hand. Answers the pause after the flip. */
export async function setAgentPause(threadId: string, paused: boolean): Promise<boolean> {
  const res = await mutatingFetch(`${API}/threads/${encodeURIComponent(threadId)}/agent-pause`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ paused }),
  });
  await throwIfNotOk(res);
  return (await res.json() as { paused: boolean }).paused;
}
