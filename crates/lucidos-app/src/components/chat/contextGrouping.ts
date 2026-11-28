import type { ContextSection } from '../../store/types';

export interface RoleGroup {
  role: 'system' | 'prior_message' | 'user';
  label: string;
  innerGroups: InnerGroup[];
}

export interface InnerGroup {
  name: string | null;
  sections: ContextSection[];
}

const TIER_ORDER = [
  'Identity & profile',
  'Workspace inventory',
  'Memory & history',
  'Loaded knowhow',
  'Active context',
  'System notices',
  'The request',
];

export function groupSections(sections: ContextSection[]): RoleGroup[] {
  const systemSections = sections.filter(s => (s.role ?? 'user') === 'system');
  const priorSections = sections.filter(s => s.role === 'prior_message');
  const userSections = sections.filter(s => (s.role ?? 'user') === 'user');

  const userInner: InnerGroup[] = [];
  const seen = new Set<string>();
  for (const tier of TIER_ORDER) {
    const inTier = userSections.filter(s => s.group === tier);
    if (inTier.length) {
      userInner.push({ name: tier, sections: inTier });
      seen.add(tier);
    }
  }
  const unknownGroups = new Set(
    userSections.filter(s => s.group && !seen.has(s.group)).map(s => s.group as string)
  );
  for (const g of [...unknownGroups].sort()) {
    userInner.push({ name: g, sections: userSections.filter(s => s.group === g) });
  }
  const ungrouped = userSections.filter(s => !s.group);
  if (ungrouped.length) userInner.push({ name: null, sections: ungrouped });

  const result: RoleGroup[] = [
    { role: 'system', label: 'System role', innerGroups: [{ name: null, sections: systemSections }] },
    { role: 'prior_message', label: 'Prior messages (verbatim replay)', innerGroups: [{ name: null, sections: priorSections }] },
    { role: 'user', label: 'User message (this turn)', innerGroups: userInner },
  ];
  return result.filter(g => g.innerGroups.some(ig => ig.sections.length > 0));
}
