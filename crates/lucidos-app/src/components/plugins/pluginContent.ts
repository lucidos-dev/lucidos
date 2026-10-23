/** Display labels for a plugin's content-dir kinds (the engine-derived
 *  `content` array: which of the engine's `CONTENT_DIRS` the plugin ships). Used by the unified plugins list (StoreTab) for the content
 *  chips on each plugin row. */
const CONTENT_LABELS: Record<string, string> = {
  apps: 'Apps',
  knowhow: 'Knowhow',
  triggers: 'Triggers',
  scripts: 'Scripts',
  'auth-modules': 'Auth',
  themes: 'Themes',
  fonts: 'Fonts',
};

export function contentLabel(kind: string): string {
  return CONTENT_LABELS[kind] ?? kind;
}
