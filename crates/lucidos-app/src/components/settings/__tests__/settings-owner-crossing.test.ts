import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, existsSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';
import { SETTINGS_NAV_ITEMS, SETTINGS_SYSTEM_SUBPANEL_ITEMS, type SettingsNavKey } from '../../../store/store';

const SETTINGS_DIR: string = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const SRC: string = resolve(SETTINGS_DIR, '../..');
const VIEW = readFileSync(resolve(SETTINGS_DIR, 'SettingsView.tsx'), 'utf8');

type Owner = 'gateway' | 'workspace';

// INVARIANT: shrink-only. Devices still merges the workspace `devices` table until
// phase 5 of the gateway and workspace settings split moves identity to the gateway.
const CROSSING_UNTIL_LATER_PHASE: ReadonlySet<SettingsNavKey> = new Set(['devices']);

const ENGINE_ONLY_CLIENT = /\$\{API\}/;
const GATEWAY_CLIENT = /['"`]\/~\/api\//;

function ownerOf(key: SettingsNavKey): Owner {
  if (SETTINGS_SYSTEM_SUBPANEL_ITEMS.some((i) => i.key === key)) return 'workspace';
  const item = SETTINGS_NAV_ITEMS.find((i) => i.key === key);
  if (!item) throw new Error(`no nav item for ${key}`);
  return item.group === 'Gateway' ? 'gateway' : 'workspace';
}

function importsOf(source: string): string[] {
  return [...source.matchAll(/^import[^'"]*from\s+['"]([^'"]+)['"]/gm)].map((m) => m[1]);
}

function resolveModule(from: string, spec: string): string | null {
  if (!spec.startsWith('.')) return null;
  const base = resolve(dirname(from), spec);
  for (const candidate of [base, `${base}.ts`, `${base}.tsx`, `${base}/index.ts`]) {
    if (candidate.endsWith('.ts') || candidate.endsWith('.tsx')) {
      if (existsSync(candidate)) return candidate;
    }
  }
  return null;
}

function clientOwner(path: string): Owner | null {
  const rel = relative(SRC, path).replace(/\\/g, '/');
  if (rel === 'api/client.ts') return 'workspace';
  if (!rel.startsWith('api/client/')) return null;
  const text = readFileSync(path, 'utf8');
  if (GATEWAY_CLIENT.test(text)) return 'gateway';
  if (ENGINE_ONLY_CLIENT.test(text)) return 'workspace';
  return null;
}

function pageEntry(key: SettingsNavKey): string | 'inline' {
  const arm = VIEW.match(new RegExp(`case '${key}': return (.+);`));
  if (!arm) throw new Error(`renderSubview draws no '${key}'`);
  const component = arm[1].match(/^<(\w+)/)?.[1];
  if (!component) return 'inline';
  const spec = VIEW.match(new RegExp(`import \\{[^}]*\\b${component}\\b[^}]*\\} from '([^']+)'`))?.[1];
  const path = spec && resolveModule(resolve(SETTINGS_DIR, 'SettingsView.tsx'), spec);
  if (!path) throw new Error(`cannot resolve ${component} for '${key}'`);
  return path;
}

function crossings(entry: string, owner: Owner): string[] {
  const found: string[] = [];
  const seen = new Set<string>();
  const stack = [entry];
  while (stack.length > 0) {
    const file = stack.pop()!;
    if (seen.has(file)) continue;
    seen.add(file);
    const text = readFileSync(file, 'utf8');
    const name = relative(SRC, file);
    if (owner === 'gateway' && ENGINE_ONLY_CLIENT.test(text)) found.push(`${name} calls \${API}`);
    if (owner === 'workspace' && GATEWAY_CLIENT.test(text)) found.push(`${name} calls /~/api`);
    for (const spec of importsOf(text)) {
      const target = resolveModule(file, spec);
      if (!target) continue;
      const client = clientOwner(target);
      if (client !== null && client !== owner) {
        found.push(`${name} imports ${relative(SRC, target)} (${client})`);
      } else if (dirname(target) === SETTINGS_DIR) {
        stack.push(target);
      }
    }
  }
  return found;
}

const KEYS: SettingsNavKey[] = [
  ...SETTINGS_NAV_ITEMS.map((i) => i.key),
  ...SETTINGS_SYSTEM_SUBPANEL_ITEMS.map((i) => i.key),
];

describe('every Settings page calls only its owner', () => {
  it.each(KEYS.filter((key) => !CROSSING_UNTIL_LATER_PHASE.has(key)))('%s', (key) => {
    const entry = pageEntry(key);
    if (entry === 'inline') {
      expect(ownerOf(key), `${key} is drawn inline by SettingsView, so it must be a workspace page`).toBe('workspace');
      return;
    }
    expect(crossings(entry, ownerOf(key))).toEqual([]);
  });

  it('keeps every exception a real, still-crossing page', () => {
    for (const key of CROSSING_UNTIL_LATER_PHASE) {
      expect(KEYS).toContain(key);
      expect(ownerOf(key)).toBe('gateway');
    }
  });

  it('can see a crossing when one exists', () => {
    expect(crossings(resolve(SETTINGS_DIR, 'MobileAccessPage.tsx'), 'workspace')).not.toEqual([]);
  });
});
