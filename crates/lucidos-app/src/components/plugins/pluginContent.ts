/** Display labels for a plugin's content-dir kinds (the engine-derived
 *  `content` array: which of apps/knowhow/triggers/scripts/auth-modules the
 *  plugin ships). Shared by the Store tab and the Installed tab so a content
 *  chip reads the same wherever it appears. */
export const CONTENT_LABELS: Record<string, string> = {
  apps: 'Apps',
  knowhow: 'Knowhow',
  triggers: 'Triggers',
  scripts: 'Scripts',
  'auth-modules': 'Auth',
};

export function contentLabel(kind: string): string {
  return CONTENT_LABELS[kind] ?? kind;
}
