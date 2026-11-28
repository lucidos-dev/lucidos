const CONTENT_LABELS: Record<string, string> = {
  apps: 'Apps',
  knowhow: 'Knowhow',
  triggers: 'Triggers',
  hooks: 'Event hooks',
  scripts: 'Scripts',
  'auth-modules': 'Auth',
  themes: 'Themes',
  fonts: 'Fonts',
};

export function contentLabel(kind: string): string {
  return CONTENT_LABELS[kind] ?? kind;
}
