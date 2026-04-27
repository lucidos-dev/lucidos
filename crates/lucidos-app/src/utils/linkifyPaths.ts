/**
 * Post-process rendered HTML to linkify artifact paths, app names, and bare URLs.
 * Tracks <a> and <code> nesting to avoid nested anchors and linkifying code content.
 */

// Cap how many alternatives go into a single regex. WebKit's YARR throws
// "Invalid regular expression: regular expression too large" on big alternations;
// V8 has no such limit. With ~50 chars per escaped path, 500 entries → ~25 KB
// source — comfortably under every engine's threshold.
const REGEX_BATCH_SIZE = 500;

const REGEX_ESCAPE = /[.*+?^${}()|[\]\\]/g;

function buildBatchedPatterns(escaped: string[], wrap: (alt: string) => string): RegExp[] {
  const patterns: RegExp[] = [];
  for (let i = 0; i < escaped.length; i += REGEX_BATCH_SIZE) {
    const batch = escaped.slice(i, i + REGEX_BATCH_SIZE);
    patterns.push(new RegExp(wrap(batch.join('|')), 'g'));
  }
  return patterns;
}

type Match = { start: number; end: number; replacement: string };

function collectMatches(text: string, patterns: RegExp[], render: (m: string) => string): Match[] {
  const matches: Match[] = [];
  for (const pattern of patterns) {
    pattern.lastIndex = 0;
    let m: RegExpExecArray | null;
    while ((m = pattern.exec(text)) !== null) {
      matches.push({ start: m.index, end: m.index + m[0].length, replacement: render(m[0]) });
    }
  }
  // Same start → longest match wins (matches single-regex alternation behavior with
  // length-desc sorted alternatives). Earlier non-overlapping match wins overall.
  matches.sort((a, b) => a.start - b.start || (b.end - b.start) - (a.end - a.start));
  const filtered: Match[] = [];
  let cursor = 0;
  for (const m of matches) {
    if (m.start >= cursor) {
      filtered.push(m);
      cursor = m.end;
    }
  }
  return filtered;
}

function applyMatches(text: string, matches: Match[]): string {
  if (matches.length === 0) return text;
  let out = '';
  let pos = 0;
  for (const m of matches) {
    out += text.slice(pos, m.start) + m.replacement;
    pos = m.end;
  }
  out += text.slice(pos);
  return out;
}

export function linkifyPaths(
  html: string,
  paths: string[],
  apps: { name: string; id: string }[],
): string {
  const segments = html.split(/(<[^>]+>)/);

  // Pre-build pattern batches outside the loop — they're invariant across segments.

  let pathPatterns: RegExp[] = [];
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
    const escaped = allMatchable.map((p) => p.replace(REGEX_ESCAPE, '\\$&'));
    pathPatterns = buildBatchedPatterns(escaped, (alt) => `(${alt})`);
  }

  let appTextToId: Map<string, string> | undefined;
  let appPatterns: RegExp[] = [];
  if (apps.length > 0) {
    // Match both app names and app IDs (LLMs sometimes use the ID)
    appTextToId = new Map();
    for (const s of apps) {
      if (!appTextToId.has(s.name)) appTextToId.set(s.name, s.id);
      if (s.id !== s.name && !appTextToId.has(s.id)) appTextToId.set(s.id, s.id);
    }
    const appTexts = [...appTextToId.keys()].sort((a, b) => b.length - a.length);
    const appEscaped = appTexts.map((t) => t.replace(REGEX_ESCAPE, '\\$&'));
    appPatterns = buildBatchedPatterns(appEscaped, (alt) => `\\b(${alt})\\b`);
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

    if (insideAnchor === 0 && pathPatterns.length > 0) {
      const matches = collectMatches(segments[i], pathPatterns, (match) => {
        const fullPath = bareToFull?.get(match) ?? match;
        const escapedPath = fullPath.replace(/"/g, '&quot;');
        return `<a class="artifact-link" data-path="${escapedPath}">${match}</a>`;
      });
      segments[i] = applyMatches(segments[i], matches);
    }

    if (insideCode > 0) continue;

    if (insideAnchor === 0 && appPatterns.length > 0) {
      const matches = collectMatches(segments[i], appPatterns, (match) => {
        const id = appTextToId?.get(match);
        if (!id) return match;
        const escapedId = id.replace(/"/g, '&quot;');
        return `<a class="app-link" data-app-id="${escapedId}">${match}</a>`;
      });
      segments[i] = applyMatches(segments[i], matches);
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
