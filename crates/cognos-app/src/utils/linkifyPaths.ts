/**
 * Post-process rendered HTML to linkify artifact paths, app names, and bare URLs.
 * Tracks <a> and <code> nesting to avoid nested anchors and linkifying code content.
 */
export function linkifyPaths(
  html: string,
  paths: string[],
  apps: { name: string; id: string }[],
): string {
  const segments = html.split(/(<[^>]+>)/);

  // Pre-build regexes outside the loop — they're invariant across segments.

  let pathPattern: RegExp | null = null;
  let bareToFull: Map<string, string> | undefined;
  if (paths.length > 0) {
    bareToFull = new Map();
    const allMatchable = [...paths];
    // Also match paths without the artifacts/ prefix (LLMs sometimes write bare paths)
    for (const p of paths) {
      if (p.startsWith('artifacts/')) {
        const bare = p.slice('artifacts/'.length);
        allMatchable.push(bare);
        bareToFull.set(bare, p);
      }
    }
    allMatchable.sort((a, b) => b.length - a.length);
    const escaped = allMatchable.map((p) => p.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
    pathPattern = new RegExp(`(${escaped.join('|')})`, 'g');
  }

  let appEntries: { text: string; id: string }[] = [];
  let appPattern: RegExp | null = null;
  if (apps.length > 0) {
    // Match both app names and app IDs (LLMs sometimes use the ID)
    const seen = new Set<string>();
    for (const s of apps) {
      if (!seen.has(s.name)) { seen.add(s.name); appEntries.push({ text: s.name, id: s.id }); }
      if (s.id !== s.name && !seen.has(s.id)) { seen.add(s.id); appEntries.push({ text: s.id, id: s.id }); }
    }
    appEntries.sort((a, b) => b.text.length - a.text.length);
    const appEscaped = appEntries.map((e) => e.text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
    appPattern = new RegExp(`\\b(${appEscaped.join('|')})\\b`, 'g');
  }

  const urlPattern = /https?:\/\/[^\s<>"')\]]+/g;

  // Track tag nesting to skip content inside <a> (prevents nested anchors)
  // and <code> (code content should not be linkified).
  let insideAnchor = 0;
  let insideCode = 0;

  for (let i = 0; i < segments.length; i++) {
    if (i % 2 === 1) {
      // Tag segment — update nesting counters
      const tag = segments[i].toLowerCase();
      if (tag.startsWith('<a ') || tag === '<a>') insideAnchor++;
      else if (tag === '</a>') insideAnchor = Math.max(0, insideAnchor - 1);
      else if (tag === '<code>' || tag.startsWith('<code ')) insideCode++;
      else if (tag === '</code>') insideCode = Math.max(0, insideCode - 1);
      continue;
    }

    // Text segment — artifact paths are linkified even inside <code> (LLMs wrap paths in backticks).
    // App names and URLs are skipped inside <code> to avoid mangling code content.

    if (insideAnchor === 0 && pathPattern) {
      pathPattern.lastIndex = 0;
      segments[i] = segments[i].replace(pathPattern, (match) => {
        // Resolve bare paths to their full store path (with artifacts/ prefix for API)
        const fullPath = bareToFull?.get(match) ?? match;
        const escapedPath = fullPath.replace(/"/g, '&quot;');
        return `<a class="artifact-link" data-path="${escapedPath}">${match}</a>`;
      });
    }

    if (insideCode > 0) continue;

    if (insideAnchor === 0 && appPattern) {
      appPattern.lastIndex = 0;
      segments[i] = segments[i].replace(appPattern, (match) => {
        const entry = appEntries.find((e) => e.text === match);
        if (!entry) return match;
        const escapedId = entry.id.replace(/"/g, '&quot;');
        return `<a class="app-link" data-app-id="${escapedId}">${match}</a>`;
      });
    }

    if (insideAnchor === 0) {
      urlPattern.lastIndex = 0;
      segments[i] = segments[i].replace(urlPattern, (match) => {
        return `<a href="${match}" target="_blank" rel="noopener">${match}</a>`;
      });
    }
  }

  return segments.join('');
}
