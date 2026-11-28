import { FOLLOW_THEME, FONT_CATALOG, isWorkspaceFontId, type FontGroup, type WorkspaceFont } from '@lucidos/appearance';
import type { DropdownOption } from '../shared/Dropdown';

export const FONT_GROUP_LABELS: Record<FontGroup, string> = {
  sans: 'Sans',
  serif: 'Serif',
  mono: 'Mono',
};

export function fontOptions(workspaceFonts: readonly WorkspaceFont[], current?: string): DropdownOption[] {
  const catalog = (Object.keys(FONT_GROUP_LABELS) as FontGroup[]).flatMap(group => [
    { value: `group:${group}`, label: FONT_GROUP_LABELS[group], disabled: true },
    ...FONT_CATALOG
      .filter(font => font.group === group && font.kind !== 'mono')
      .map(font => ({ value: font.id, label: font.label })),
  ]);
  const installed = workspaceFonts.map(font => ({ value: font.id, label: font.label }));
  const missing = isWorkspaceFontId(current) && !workspaceFonts.some(font => font.id === current)
    ? [{ value: current, label: `${current} (not installed)` }]
    : [];
  const rows = [...installed, ...missing];
  const workspace = rows.length === 0 ? [] : [
    { value: 'group:workspace', label: 'Workspace', disabled: true },
    ...rows,
  ];
  return [{ value: FOLLOW_THEME, label: 'Follow the theme' }, ...catalog, ...workspace];
}
