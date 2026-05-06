export function getEmojiForFile(path: string): string {
  if (path.endsWith('.csv')) return '📊';
  if (path.endsWith('.md')) return '📄';
  if (path.endsWith('.json')) return '📋';
  if (path.endsWith('.py')) return '🐍';
  if (path.endsWith('.txt')) return '📝';
  if (path.endsWith('.html')) return '🌐';
  if (path.endsWith('.pdf')) return '📕';
  if (
    path.endsWith('.png') ||
    path.endsWith('.jpg') ||
    path.endsWith('.jpeg') ||
    path.endsWith('.gif') ||
    path.endsWith('.webp')
  )
    return '🖼';
  if (path.endsWith('.js') || path.endsWith('.ts')) return '📜';
  if (path.endsWith('.css')) return '🎨';
  if (path.includes('report')) return '📈';
  return '📄';
}