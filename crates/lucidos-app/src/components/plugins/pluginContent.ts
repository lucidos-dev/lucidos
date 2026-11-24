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
