import type { UserListing } from '../../api/client';
import { AGENT_MENTION } from '../../store/actions/users';

export function mentionQueryAt(value: string, caret: number): { query: string; start: number } | null {
  const match = /(^|\s)@([^\s@]*)$/u.exec(value.slice(0, caret));
  if (!match) return null;
  return { query: match[2], start: caret - match[2].length - 1 };
}

export interface MentionCandidate {
  token: string;
  label: string;
  hint?: string;
}

export function mentionCandidates(query: string, users: readonly UserListing[], agentName: string): MentionCandidate[] {
  const q = query.toLowerCase();
  const matches = (name: string) => !q || name.toLowerCase().split(/\s+/).some((word) => word.startsWith(q));
  const out: MentionCandidate[] = users
    .filter((p) => matches(p.name))
    .map((p) => ({ token: p.name, label: `@${p.name}`, hint: p.has_device ? undefined : 'no device' }));
  if (matches(AGENT_MENTION) || matches(agentName)) {
    out.push({ token: AGENT_MENTION, label: `@${AGENT_MENTION}`, hint: agentName });
  }
  return out;
}
