import { describe, it, expect } from 'vitest';
import { exemptionProblems, rawSourceOf, scanSources, type Exemption } from './loading-guard-scan';

/**
 * Every async read lands in a `Loadable<T>` (`.claude/rules/frontend.md`
 * § "Async Data Loading"). A bare `T | null` or `T[]` holding a fetch reads
 * as "empty" while it loads and after it fails. Source scans, default-deny:
 *
 *  - A file that imports an API read must also hold it in a `Loadable`.
 *  - No component keeps a `loading` flag in `useState` beside its data.
 *
 * File level on purpose: a file that holds one read in a `Loadable` and a
 * second one bare passes. Review covers that; the guard stops the common shape.
 */

/** The API read verbs. Writes (`save*`, `create*`, `delete*`) are actions. */
const API_READ = /^(get|fetch|list|search|browse)[A-Z]/;

function apiReadsImported(code: string): string[] {
  const reads: string[] = [];
  for (const [, names] of code.matchAll(/import\s*\{([^}]*)\}\s*from\s*'(?:\.\.?\/)+api\/[^']*'/g)) {
    for (const raw of names.split(',')) {
      const name = raw.trim().replace(/\s+as\s+\w+$/, '');
      if (!name.startsWith('type ') && API_READ.test(name)) reads.push(name);
    }
  }
  return reads;
}

function holdsLoadable(code: string): boolean {
  return /\bLoadable\b|\buseLoadableFetch\b|\btoFailed\b|status:\s*'loaded'|IfFresh\b/.test(code);
}

function hasLoadingFlagState(code: string): boolean {
  return /\[\s*\w*[lL]oading\w*\s*,\s*set\w*\s*\]\s*=\s*useState\b/.test(code);
}

/** Reads that may stay outside a `Loadable`, each with the rule that says so. */
const READ_EXEMPT: Record<string, Exemption> = {
  'components/credentials/CredentialItem.tsx': { tag: 'returns-value', why: 'copies a secret on a press; failure toasts' },
  'components/settings/SettingsView.tsx': { tag: 'returns-value', why: 'hands the read to AllowlistEditor, which holds the Loadable' },
  'store/actions/app-badge.ts': { tag: 'best-effort', why: 'cross-workspace unread count for the icon badge' },
  'store/actions/backgroundActivity.ts': { tag: 'best-effort', why: 'embedding download progress probe; SSE carries the live truth' },
  'store/actions/cross-workspace.ts': { tag: 'returns-value', why: 'resolves a peer workspace URL for its caller' },
  'store/actions/event-navigation.ts': { tag: 'returns-value', why: 'looks up the thread owning an event for a jump' },
  'store/actions/form-requests.ts': { tag: 'best-effort', why: 'offers an open form request on each stream open' },
  'store/actions/slowness.ts': { tag: 'best-effort', why: 'a slowness banner that is simply absent when unmeasurable' },
  'store/actions/update-relay.ts': { tag: 'best-effort', why: 'update relay status rides the release check; a watch has its own give-up bound' },
  'store/actions/workspace-label.ts': { tag: 'best-effort', why: 'startup probe for the display name, with a fallback' },
};

const sources = ['components', 'store', 'hooks'].flatMap((d) => scanSources(d, ['.ts', '.tsx']));

describe('loadable guard scanners', () => {
  it('finds API reads among the imports, and skips writes and types', () => {
    const code = "import { getThing, saveThing, type ListResult, listRows as rows } from '../../api/client';";
    expect(apiReadsImported(code)).toEqual(['getThing', 'listRows']);
    expect(apiReadsImported("import { getThing } from '../store/store';")).toEqual([]);
  });

  it('spots a loading flag held beside the data', () => {
    expect(hasLoadingFlagState('const [loading, setLoading] = useState(false);')).toBe(true);
    expect(hasLoadingFlagState('const [isLoadingMore, setIsLoadingMore] = useState(false);')).toBe(true);
    expect(hasLoadingFlagState('const [saving, setSaving] = useState(false);')).toBe(false);
  });

  it('recognises a Loadable holder', () => {
    expect(holdsLoadable("setRows({ status: 'loaded', data })")).toBe(true);
    expect(holdsLoadable('const [rows, setRows] = useState<Row[] | null>(null);')).toBe(false);
  });
});

describe('loadable guard', () => {
  it('holds every API read in a Loadable', () => {
    const bareReads = new Set(sources.filter((f) => apiReadsImported(f.code).length > 0 && !holdsLoadable(f.code)).map((f) => f.path));
    const flags = new Set(sources.filter((f) => f.path.startsWith('components/') && hasLoadingFlagState(f.code)).map((f) => f.path));
    const all = new Set([...bareReads, ...flags]);

    // A loading flag is never excused; a bare read may be.
    const excused = (p: string) => !!READ_EXEMPT[p] && !flags.has(p);
    expect([...all].filter((p) => !excused(p)), 'hold the read in a Loadable<T> (frontend.md § Async Data Loading)').toEqual([]);
    expect([...exemptionProblems(READ_EXEMPT, bareReads, rawSourceOf)]).toEqual([]);
  });
});
