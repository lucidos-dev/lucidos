// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, join, relative } from 'node:path';
import { VOICE_RESIDENT_SECTIONS } from '../../../store/actions/preferences';
import { providerAnchor } from '../../../store/modelProviders';
import { SYSTEM_ONE_PROVIDERS } from '../../settings/judgmentBackend';
import { LADDER_SURFACES, ladderAnchor } from '../../settings/intelligenceLadder';
import { SHORTCUT_DEFS, shortcutSearchAnchor } from '../../../utils/shortcuts';

const COMPONENTS_DIR = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');

type DynamicAnchor =
  | { kind: 'forwarded' }
  | { kind: 'expands'; anchors: () => string[] }
  | { kind: 'not-a-setting'; why: string };

const systemOneFrameAnchors = () =>
  Object.values(SYSTEM_ONE_PROVIDERS).filter((p) => !p.chatProvider).map((p) => p.anchor);

const DYNAMIC_ANCHORS: Record<string, DynamicAnchor> = {
  'settings/AllowlistEditor.tsx props.anchor': { kind: 'forwarded' },
  'settings/ModelSelectionRow.tsx anchor': { kind: 'forwarded' },
  'settings/BackgroundModelRow.tsx anchor': { kind: 'forwarded' },
  'settings/JudgmentModelRow.tsx props.anchor': { kind: 'forwarded' },
  'settings/ProviderBlock.tsx props.anchor': { kind: 'forwarded' },
  'settings/ProviderBlockFrame.tsx props.anchor': { kind: 'forwarded' },
  'settings/ApiKeyProviderSettings.tsx providerAnchor(service)': {
    kind: 'expands',
    anchors: () => componentSources()
      .filter(({ src }) => src.includes('<ApiKeyProviderSettings'))
      .flatMap(({ src }) => [...src.matchAll(/\bservice="([^"]+)"/g)].map((m) => providerAnchor(m[1]))),
  },
  'settings/SystemOneProviderFrame.tsx spec.anchor': { kind: 'expands', anchors: systemOneFrameAnchors },
  'settings/SystemOneProviderFrame.tsx props.anchor': { kind: 'forwarded' },
  'settings/SystemOneProviderFrame.tsx `${spec.anchor}-usage`': {
    kind: 'expands',
    anchors: () => systemOneFrameAnchors().map((a) => `${a}-usage`),
  },
  "settings/ApiKeyProviderSettings.tsx providerAnchor(systemOne, 'usage')": {
    kind: 'expands',
    anchors: () => Object.values(SYSTEM_ONE_PROVIDERS)
      .filter((p) => p.chatProvider)
      .map((p) => `${p.anchor}-usage`),
  },
  'settings/VoiceSection.tsx `models:voice-section-${section.id}`': {
    kind: 'expands',
    anchors: () => VOICE_RESIDENT_SECTIONS.map((s) => `models:voice-section-${s.id}`),
  },
  'settings/IntelligencePage.tsx ladderAnchor(surface)': {
    kind: 'expands',
    anchors: () => LADDER_SURFACES.map(({ surface }) => ladderAnchor(surface)),
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

const LITERAL_PROVIDER_ANCHOR = /^providerAnchor\('([^']+)'(?:, '([^']+)')?\)$/;

function literalProviderAnchor(expression: string): string | null {
  const m = LITERAL_PROVIDER_ANCHOR.exec(expression);
  return m ? providerAnchor(m[1], m[2]) : null;
}

function dynamicExpressions(): string[] {
  const keys: string[] = [];
  for (const { path, src } of componentSources()) {
    const patterns = [/data-search-anchor=\{(`[^`]*`|[^}]*)\}/g];
    if (path.startsWith('settings/')) patterns.push(/\banchor=\{(`[^`]*`|[^}]*)\}/g);
    for (const re of patterns) {
      for (const m of src.matchAll(re)) {
        if (!literalProviderAnchor(m[1].trim())) keys.push(`${path} ${m[1].trim()}`);
      }
    }
  }
  return [...new Set(keys)];
}

export function renderedSearchAnchors(): Set<string> {
  const anchors = new Set<string>();
  for (const { src } of componentSources()) {
    for (const m of src.matchAll(/\b[\w-]*[Aa]nchor="([^"$]+)"/g)) anchors.add(m[1]);
    for (const m of src.matchAll(/\b[\w-]*[Aa]nchor=\{([^}]*)\}/g)) {
      const anchor = literalProviderAnchor(m[1].trim());
      if (anchor) anchors.add(anchor);
    }
  }
  for (const row of Object.values(DYNAMIC_ANCHORS)) {
    if (row.kind === 'expands') row.anchors().forEach((a) => anchors.add(a));
  }
  return anchors;
}

export function unclassifiedDynamicAnchors(): string[] {
  return dynamicExpressions().filter((key) => !(key in DYNAMIC_ANCHORS));
}

export function staleDynamicAnchorRows(): string[] {
  const live = new Set(dynamicExpressions());
  return Object.keys(DYNAMIC_ANCHORS).filter((key) => !live.has(key));
}
