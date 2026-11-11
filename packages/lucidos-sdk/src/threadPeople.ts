import { request } from './_fetch';

/** A person, as a role in a thread names them. `name` reads "Sam via A" for
 *  someone reached through a gateway pairing. */
export interface OperatorRef {
  id: string;
  name: string;
}

/** Who steers a root thread and its sub-threads (ADR 0623). Roles are never
 *  authority: anyone can still answer any gate. */
export interface ThreadPeople {
  root_thread_id: string;
  driver: OperatorRef | null;
  watchers: OperatorRef[];
}

/** One open root thread in the factory listing. `needs_team` is the Team
 *  view: an open gate with no driver, or one that escalated. `yours` marks
 *  the ones the calling device's person drives or watches. */
export interface ThreadPeopleSummary {
  root_thread_id: string;
  title: string | null;
  last_activity: string;
  driver: OperatorRef | null;
  watchers: OperatorRef[];
  gate_open: boolean;
  escalated: boolean;
  needs_team: boolean;
  yours: boolean;
}

/** The open root threads, newest activity first, and the caller's person
 *  (`null` for a device no person holds). */
export interface ThreadPeopleList {
  items: ThreadPeopleSummary[];
  person: OperatorRef | null;
}

function peoplePath(threadId: string): string {
  return `/threads/${encodeURIComponent(threadId)}/people`;
}

function change(threadId: string, verb: string): Promise<ThreadPeople> {
  return request<ThreadPeople>(`${peoplePath(threadId)}/${verb}`, { method: 'POST' });
}

/** The driver and watchers of a root thread. Any thread id names its root.
 *  Each change acts as the person holding this device, and returns the
 *  people after it. A device that belongs to no person gets a 409. See
 *  system-knowhow/js-sdk.md § lucidos.threads.people. */
export const people = {
  /** The open root threads: sent and not archived. */
  list(): Promise<ThreadPeopleList> {
    return request<ThreadPeopleList>('/threads/people');
  },

  /** The root thread's driver and watchers. */
  get(threadId: string): Promise<ThreadPeople> {
    return request<ThreadPeople>(peoplePath(threadId));
  },

  /** Drive the root thread. On one someone else drives, this is Take over. */
  take(threadId: string): Promise<ThreadPeople> {
    return change(threadId, 'take');
  },

  /** Stop driving. Changes nothing when you do not drive it. */
  release(threadId: string): Promise<ThreadPeople> {
    return change(threadId, 'release');
  },

  /** Watch: its gate pushes reach you beside the driver. */
  watch(threadId: string): Promise<ThreadPeople> {
    return change(threadId, 'watch');
  },

  /** Stop watching, from any of your devices. */
  unwatch(threadId: string): Promise<ThreadPeople> {
    return change(threadId, 'unwatch');
  },
};
