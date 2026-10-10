import type { FootprintItem, FootprintSection, WorkspacePromptFootprint } from '../../api/client';
import { countOf } from '../../utils/recommendedCleanup';

/** What the Run audit button sends. It names the audit and the section, so it
 *  routes to the audit root and runs only the prompt-footprint section.
 *  `the_footprint_page_prompt_routes_to_the_audit_root` pins the routing. */
export const PROMPT_FOOTPRINT_AUDIT_PROMPT = "Audit my workspace's prompt footprint.";

/** How many items the Largest tab lists. */
export const LARGEST_ITEMS_SHOWN = 8;

export type ItemTab = 'largest' | 'clipped' | 'unused';

/** The sections that cost anything, largest first. */
export function costlySections(report: WorkspacePromptFootprint): FootprintSection[] {
  return report.sections.filter((s) => s.chars > 0).sort((a, b) => b.chars - a.chars);
}

/** A meter's fill, as a share of the ceiling, capped at full. */
export function meterFraction(chars: number, ceiling: number): number {
  if (ceiling <= 0) return 1;
  return Math.min(1, chars / ceiling);
}

function allItems(report: WorkspacePromptFootprint): FootprintItem[] {
  return report.sections.flatMap((s) => s.items);
}

/** Whether an item goes on the Unused tab. A knowhow doc is never proven
 *  unused, since a shell read leaves no trace, so it is "not loaded by name". */
export function looksUnused(item: FootprintItem): boolean {
  const verdict = item.usage?.verdict;
  return verdict === 'unused' || verdict === 'not-loaded-by-name';
}

/** The items each tab lists. */
export function itemsFor(report: WorkspacePromptFootprint, tab: ItemTab): FootprintItem[] {
  const items = allItems(report);
  switch (tab) {
    case 'largest':
      return [...items].sort((a, b) => b.chars - a.chars).slice(0, LARGEST_ITEMS_SHOWN);
    case 'clipped':
      return items.filter((i) => i.clipped_chars > 0).sort((a, b) => b.clipped_chars - a.clipped_chars);
    case 'unused':
      return items.filter(looksUnused);
  }
}

/** The one line the Run audit card leads with. */
export function findingsLine(report: WorkspacePromptFootprint): string {
  const over = report.sections.filter((s) => s.over_ceiling).length;
  const clipped = itemsFor(report, 'clipped').length;
  const flagged = itemsFor(report, 'unused');
  const unused = flagged.filter((i) => i.usage?.verdict === 'unused').length;
  const unread = flagged.length - unused;
  const parts = [
    over > 0 && `${countOf(over, 'section')} over its ceiling`,
    report.over_total_ceiling && 'the total over its ceiling',
    clipped > 0 && `${clipped} clipped`,
    unused > 0 && `${unused} unused`,
    unread > 0 && `${unread} not loaded by name`,
  ].filter(Boolean);
  if (parts.length === 0) return 'Nothing over a ceiling, clipped or unused';
  const line = parts.join(', ');
  return line.charAt(0).toUpperCase() + line.slice(1);
}

const KIND_LABELS: Record<FootprintItem['kind'], string> = {
  app: 'App',
  'reusable-widget': 'Reusable widget',
  knowhow: 'Know-how',
  intent: 'Intent',
  'response-style': 'Response style',
  'user-profile': 'Profile',
  'email-account': 'Email account',
  'oauth-account': 'OAuth account',
  credential: 'Credential',
  'mcp-server': 'MCP server',
  'mcp-tool': 'MCP tool',
  'app-knowhow': 'App know-how listing',
};

export function kindLabel(kind: FootprintItem['kind']): string {
  return KIND_LABELS[kind];
}

/** When an item was last used, for its row. */
export function usageLine(item: FootprintItem): string | null {
  const usage = item.usage;
  if (!usage) return null;
  const verb = item.kind === 'knowhow' ? 'loaded' : item.kind === 'reusable-widget' ? 'shown' : 'opened';
  const days = usage.last_used_days_ago;
  if (usage.verdict === 'not-loaded-by-name' && days === null) return 'not loaded by name';
  if (days === null) return usage.verdict === 'unused' ? `never ${verb}` : null;
  const when = days === 0 ? 'today' : `${countOf(days, 'day')} ago`;
  return `${verb} ${when}`;
}
