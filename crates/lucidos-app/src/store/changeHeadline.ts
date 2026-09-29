/** What names a change on the card, in a toast and in the Changes panel.
 *
 *  A change's `description` is its commit subjects, NEWEST first, one per line.
 *  So its first line is the latest commit, typically a small fix, and never the
 *  thing to title a change of several commits with. The *change summary* is,
 *  once a model has written it; until then the OLDEST subject stands in, being
 *  usually the main piece of work. */
export interface ChangeNaming {
  description?: string | null;
  summary?: string | null;
  /** Subjects merged to main, oldest first. Set once the change is applied. */
  commits?: readonly string[] | null;
}

/** Every commit subject, oldest first. The applied list when there is one,
 *  since it is what actually landed; else the description, reversed. */
export function changeCommitList(change: ChangeNaming): string[] {
  if (change.commits && change.commits.length > 0) return [...change.commits];
  return (change.description ?? '')
    .split('\n')
    .map(line => line.trim())
    .filter(line => line.length > 0)
    .reverse();
}

/** The one line naming a change. Never the newest subject of several. */
export function changeHeadline(change: ChangeNaming): string {
  return change.summary?.trim() || changeCommitList(change)[0] || 'Change';
}

/** The event fields this reads. Loose, so any thread event type fits. */
interface NamingEvent {
  type: string;
  change_id?: string;
  description?: string;
  summary?: string;
}

/** A change's naming from a thread's own events: the latest proposal, and the
 *  summary of that commit list. Undefined when no proposal is in hand. The same
 *  guard the engine's projection applies, so an older summary never names it. */
export function changeNamingFromEvents(events: Iterable<NamingEvent>, changeId: string): ChangeNaming | undefined {
  let description: string | undefined;
  const summaries = new Map<string, string>();
  for (const event of events) {
    if (event.change_id !== changeId) continue;
    if (event.type === 'ChangeProposed' && event.description) {
      description = event.description;
    } else if (event.type === 'ChangeSummarized' && event.summary) {
      summaries.set(event.description ?? '', event.summary);
    }
  }
  return description === undefined ? undefined : { description, summary: summaries.get(description) };
}
