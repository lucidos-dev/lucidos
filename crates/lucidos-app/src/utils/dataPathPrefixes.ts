export const DATA_PATH_PREFIXES: readonly string[] = [
  'artifacts/',
  'apps/',
  'knowhow/',
  'triggers/',
  'config/',
  'auth-modules/',
  'scripts/',
  'themes/',
  'fonts/',
  'system-knowhow/',
];

export function workspaceDataRoot(workspacePath: string): string {
  return `${workspacePath.replace(/\/+$/, '')}/data/`;
}
