/** Display labels for a plugin's content-dir kinds (the engine-derived
 *  `content` array: which of apps/knowhow/triggers/scripts/auth-modules the
 *  plugin ships). Used by the unified plugins list (StoreTab) for the content
 *  chips on each plugin row. */
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
