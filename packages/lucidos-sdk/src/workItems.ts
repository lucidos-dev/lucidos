import { request } from './_fetch';

/** A person, as a work item role names them. `name` reads "Sam via A" for
 *  someone reached through a gateway pairing. */
export interface OperatorRef {
  id: string;
  name: string;
}

/** Who steers a work item: a root thread and its sub-threads (ADR 0623).
 *  Roles are never authority: anyone can still answer any gate. */
export interface WorkItemRoles {
  root_thread_id: string;
  driver: OperatorRef | null;
  watchers: OperatorRef[];
}

/** One open work item in the factory listing. `needs_team` is the Team view:
 *  an open gate with no driver, or one that escalated. `yours` marks the
 *  ones the calling device's person drives or watches. */
export interface WorkItemSummary {
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

/** The open work items, newest activity first, and the caller's person
 *  (`null` for a device no person holds). */
export interface WorkItemList {
  items: WorkItemSummary[];
  person: OperatorRef | null;
}

function change(threadId: string, verb: string): Promise<WorkItemRoles> {
  return request<WorkItemRoles>(
    `/work-items/${encodeURIComponent(threadId)}/${verb}`,
    { method: 'POST' },
  );
}

/** Work item roles. Any thread id names its work item. Each change acts as
 *  the person holding this device, and returns the roles after it. A device
 *  that belongs to no person gets a 409. See system-knowhow/js-sdk.md
 *  § lucidos.workItems. */
export const workItems = {
  /** The open work items: root threads not archived. */
  list(): Promise<WorkItemList> {
    return request<WorkItemList>('/work-items');
  },

  /** The work item's driver and watchers. */
  get(threadId: string): Promise<WorkItemRoles> {
    return request<WorkItemRoles>(`/work-items/${encodeURIComponent(threadId)}`);
  },

  /** Drive the work item. On one someone else drives, this is Take over. */
  take(threadId: string): Promise<WorkItemRoles> {
    return change(threadId, 'take');
  },

  /** Stop driving. Changes nothing when you do not drive it. */
  release(threadId: string): Promise<WorkItemRoles> {
    return change(threadId, 'release');
  },

  /** Watch: its gate pushes reach you beside the driver. */
  watch(threadId: string): Promise<WorkItemRoles> {
    return change(threadId, 'watch');
  },

  /** Stop watching, from any of your devices. */
  unwatch(threadId: string): Promise<WorkItemRoles> {
    return change(threadId, 'unwatch');
  },
};
