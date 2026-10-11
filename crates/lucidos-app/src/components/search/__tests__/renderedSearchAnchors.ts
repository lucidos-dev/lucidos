/**
 * Every Settings search anchor the components render, read from source.
 *
 * Shared by the two guards that tie Settings to Search everywhere: every
 * search entry lands on a rendered anchor, and every rendered anchor has a
 * search entry. One scan for both, so the two directions cannot disagree.
 *
 * A literal anchor is read straight off the markup. An anchor built at runtime
 * cannot be, so each such expression needs a row in `DYNAMIC_ANCHORS` saying
 * where its values come from. A new expression with no row fails the guard.
 */
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, join, relative } from 'node:path';
import { VOICE_RESIDENT_SECTIONS } from '../../../store/actions/preferences';
import { SYSTEM_ONE_PROVIDERS } from '../../settings/judgmentBackend';
import { SHORTCUT_DEFS, shortcutSearchAnchor } from '../../../utils/shortcuts';

const COMPONENTS_DIR = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');

type DynamicAnchor =
  /** A prop. Its callers pass the anchor, and the scan reads it there. */
  | { kind: 'forwarded' }
  /** Built from a list. These are every value it can take. */
  | { kind: 'expands'; anchors: () => string[] }
  /** Not a Settings row, so it needs no search entry. */
  | { kind: 'not-a-setting'; why: string };

const systemOneAnchors = () => Object.values(SYSTEM_ONE_PROVIDERS).map((p) => p.anchor);

/** Keyed by `<path under components/> <expression>`. */
const DYNAMIC_ANCHORS: Record<string, DynamicAnchor> = {
  'settings/AllowlistEditor.tsx props.anchor': { kind: 'forwarded' },
  'settings/ModelSelectionRow.tsx anchor': { kind: 'forwarded' },
  'settings/BackgroundModelRow.tsx anchor': { kind: 'forwarded' },
  'settings/JudgmentModelRow.tsx props.anchor': { kind: 'forwarded' },
  'settings/ProviderBlock.tsx props.anchor': { kind: 'forwarded' },
  'settings/ProviderBlockFrame.tsx props.anchor': { kind: 'forwarded' },
  'settings/ApiKeyProviderSettings.tsx `models:${service}`': {
    kind: 'expands',
    anchors: () => componentSources()
      .filter(({ src }) => src.includes('<ApiKeyProviderSettings'))
      .flatMap(({ src }) => [...src.matchAll(/\bservice="([^"]+)"/g)].map((m) => `models:${m[1]}`)),
  },
  'settings/SystemOneProviderFrame.tsx spec.anchor': { kind: 'expands', anchors: systemOneAnchors },
  'settings/SystemOneProviderFrame.tsx `${spec.anchor}-usage`': {
    kind: 'expands',
    anchors: () => systemOneAnchors().map((a) => `${a}-usage`),
  },
  'settings/VoiceSection.tsx `models:voice-section-${section.id}`': {
    kind: 'expands',
    anchors: () => VOICE_RESIDENT_SECTIONS.map((s) => `models:voice-section-${s.id}`),
  },
  'settings/KeyboardShortcutsSection.tsx shortcutSearchAnchor(def.id)': {
    kind: 'expands',
    anchors: () => SHORTCUT_DEFS.map((d) => shortcutSearchAnchor(d.id)),
  },
  'credentials/CredentialItem.tsx credentialAnchor(credential.id)': {
    kind: 'not-a-setting',
    why: 'one row per stored credential, which is user data; the Credentials section entry finds the list',
  },
  'settings/MemoryModuleSection.tsx treeButton': {
    kind: 'not-a-setting',
    why: 'the element the backfill confirm hangs from, not a search anchor',
  },
  'settings/TreeBackfillConfirm.tsx anchor': {
    kind: 'not-a-setting',
    why: 'the same element, passed on to its Overlay',
  },
};

let sources: Array<{ path: string; src: string }> | null = null;

/** Every non-test `.tsx` under `components/`, keyed by its path there. */
function componentSources(): Array<{ path: string; src: string }> {
  if (sources) return sources;
  const found: Array<{ path: string; src: string }> = [];
  const walk = (dir: string) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) {
        if (entry.name !== '__tests__') walk(path);
      } else if (entry.name.endsWith('.tsx')) {
        found.push({ path: relative(COMPONENTS_DIR, path), src: readFileSync(path, 'utf8') });
      }
    }
  };
  walk(COMPONENTS_DIR);
  return (sources = found);
}

/** `data-search-anchor={…}` anywhere, plus an `anchor={…}` prop in Settings,
 *  where the row components that render the attribute take it. */
function dynamicExpressions(): string[] {
  const keys: string[] = [];
  for (const { path, src } of componentSources()) {
    const patterns = [/data-search-anchor=\{(`[^`]*`|[^}]*)\}/g];
    if (path.startsWith('settings/')) patterns.push(/\banchor=\{(`[^`]*`|[^}]*)\}/g);
    for (const re of patterns) {
      for (const m of src.matchAll(re)) keys.push(`${path} ${m[1].trim()}`);
    }
  }
  return [...new Set(keys)];
}

/** Every anchor a Settings row can render. */
export function renderedSearchAnchors(): Set<string> {
  const anchors = new Set<string>();
  for (const { src } of componentSources()) {
    // `data-search-anchor="…"`, and any `*anchor="…"` prop a row component
    // turns into the attribute for its caller. A `${…}` value is a selector
    // inside a template string, not markup.
    for (const m of src.matchAll(/\b[\w-]*[Aa]nchor="([^"$]+)"/g)) anchors.add(m[1]);
  }
  for (const row of Object.values(DYNAMIC_ANCHORS)) {
    if (row.kind === 'expands') row.anchors().forEach((a) => anchors.add(a));
  }
  return anchors;
}

/** Runtime-built anchor expressions with no `DYNAMIC_ANCHORS` row. */
export function unclassifiedDynamicAnchors(): string[] {
  return dynamicExpressions().filter((key) => !(key in DYNAMIC_ANCHORS));
}

/** `DYNAMIC_ANCHORS` rows whose expression no longer appears in source. */
export function staleDynamicAnchorRows(): string[] {
  const live = new Set(dynamicExpressions());
  return Object.keys(DYNAMIC_ANCHORS).filter((key) => !live.has(key));
}
