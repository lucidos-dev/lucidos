export interface ChangeNaming {
  description?: string | null;
  summary?: string | null;
  commits?: readonly string[] | null;
}

export function changeCommitList(change: ChangeNaming): string[] {
  if (change.commits && change.commits.length > 0) return [...change.commits];
  return (change.description ?? '')
    .split('\n')
    .map(line => line.trim())
    .filter(line => line.length > 0)
    .reverse();
}

export function changeHeadline(change: ChangeNaming): string {
  return knownChangeHeadline(change) ?? 'Change';
}

export function knownChangeHeadline(change: ChangeNaming): string | undefined {
  return change.summary?.trim() || changeCommitList(change)[0] || undefined;
}

interface NamingEvent {
  type: string;
  change_id?: string;
  description?: string;
  summary?: string;
}

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
