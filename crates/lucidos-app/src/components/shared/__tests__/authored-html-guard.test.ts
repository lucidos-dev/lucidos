import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';

/**
 * Every host element that receives sanitizer output carries `.authored-html`,
 * whose clip keeps authored paint and hit-testing inside its box
 * (`styles/global/host-components.css`). Without it, an inline
 * `position: fixed` or a host class like `modal-overlay` covers the whole app.
 *
 * Default-deny: every `dangerouslySetInnerHTML` in `components/` names the
 * class in its own opening tag, or takes an exemption below with its reason.
 */

const here = dirname(fileURLToPath(import.meta.url));
const COMPONENTS = resolve(here, '../..');

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name !== '__tests__') out.push(...sourceFiles(full));
    } else if (entry.name.endsWith('.tsx') && !entry.name.endsWith('.test.tsx')) {
      out.push(full);
    }
  }
  return out;
}

/** Sites whose HTML is escaped by construction, never sanitizer output, or
 *  contained by an ancestor. Keyed by file and opening tag name. */
const EXEMPT: Record<string, string> = {
  'files/DiffView.tsx span': 'highlighter spans over escaped source text',
  'files/LineNumberedCode.tsx span': 'highlighter spans over escaped source text',
  'files/FilePreviewInline.tsx div': 'CSV table built from escaped cells',
  'files/RepoFilePreview.tsx div': 'CSV table built from escaped cells',
  'files/SlidesPreview.tsx li': 'clipped by its `<ul class="slides-pv-list authored-html">`, which leaves the marker room',
};

interface Site { key: string; tag: string }

function injectionSites(): Site[] {
  const sites: Site[] = [];
  for (const file of sourceFiles(COMPONENTS)) {
    const src: string = readFileSync(file, 'utf8');
    const rel = relative(COMPONENTS, file);
    for (const match of src.matchAll(/dangerouslySetInnerHTML=\{/g)) {
      const before = src.slice(0, match.index);
      const open = Math.max(...Array.from(before.matchAll(/<[A-Za-z][\w.]*/g), (m) => m.index ?? -1));
      const tag = src.slice(open, match.index);
      const name = /^<([\w.]+)/.exec(tag)?.[1] ?? '?';
      sites.push({ key: `${rel} ${name}`, tag });
    }
  }
  return sites;
}

describe('authored HTML is contained', () => {
  const sites = injectionSites();

  it('finds the injection sites', () => {
    expect(sites.length).toBeGreaterThan(20);
  });

  it('gives every injection site the containment class, or an exemption', () => {
    const bare = sites
      .filter((site) => !/\bauthored-html\b/.test(site.tag) && !(site.key in EXEMPT))
      .map((site) => site.key);
    expect(bare).toEqual([]);
  });

  it('keeps no stale exemption', () => {
    const keys = new Set(sites.map((site) => site.key));
    expect(Object.keys(EXEMPT).filter((key) => !keys.has(key))).toEqual([]);
  });
});
