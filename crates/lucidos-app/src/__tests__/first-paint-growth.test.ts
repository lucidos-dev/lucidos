/**
 * The first-paint growth check (ADR 0454). A change's growth is measured
 * against a build of its merge base, and growth over the threshold needs a
 * reason. The nightly ranking reads main's reflog.
 *
 * Importing the tool is safe: it runs a command only when it IS the entry
 * module. The builds are injected, so nothing here runs Vite. The baseline
 * cases use real git, in throwaway repositories. The registry cases run real
 * `npm config get`, which reads config files and never the network.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import * as growth from '../../first-paint-growth.mjs';
import type { Build, FirstPaintReport } from '../../first-paint-growth.mjs';
import { FIRST_PAINT_BUDGET, FIRST_PAINT_REPORT_FILE } from '../../vite/chunkBudget';
import { envWith, join, makeRepo, readIfPresent, removeDir, tempDir, writeAt, type Sandbox } from './gitSandbox.mjs';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { mkdirSync } from 'node:fs';
// @ts-expect-error: same
import { dirname } from 'node:path';

const { GROWTH_THRESHOLD_BYTES, GROWTH_TRAILER } = growth;

function report(setBytes: number, chunks?: FirstPaintReport['chunks']): FirstPaintReport {
  return {
    entryBytes: 500_000,
    setBytes,
    setGzipBytes: Math.round(setBytes / 3),
    budgetBytes: FIRST_PAINT_BUDGET.kb * 1000,
    chunks: chunks ?? [{ file: 'assets/index.js', bytes: setBytes }],
  };
}

const BASE = report(1_200_000);
const OVER = GROWTH_THRESHOLD_BYTES + 1;

describe('growthVerdict', () => {
  it('fails growth over the threshold that carries no reason', () => {
    const verdict = growth.growthVerdict(BASE, report(BASE.setBytes + OVER), []);
    expect(verdict.kind).toBe('fail');
    expect(verdict.deltaBytes).toBe(OVER);
    expect(verdict.message).toContain(`${GROWTH_TRAILER}:`);
    expect(verdict.message).toContain(`+${(OVER / 1000).toFixed(2)} kB`);
  });

  it('passes growth over the threshold that carries a reason, and prints it', () => {
    const verdict = growth.growthVerdict(BASE, report(BASE.setBytes + OVER), ['The thread view draws the new status line.']);
    expect(verdict.kind).toBe('reasoned');
    expect(verdict.message).toContain('The thread view draws the new status line.');
  });

  it('passes growth up to the threshold without a word', () => {
    expect(growth.growthVerdict(BASE, report(BASE.setBytes + GROWTH_THRESHOLD_BYTES), []).kind).toBe('within');
  });

  it('passes a shrink, which needs no reason and shows negative', () => {
    const verdict = growth.growthVerdict(BASE, report(BASE.setBytes - 5_432), []);
    expect(verdict.kind).toBe('shrink');
    expect(verdict.deltaBytes).toBe(-5_432);
    expect(verdict.message).toContain('-5.43 kB');
  });

  it('counts bytes moved between first-paint chunks as no growth', () => {
    const base = report(1_200_000, [{ file: 'assets/App-a.js', bytes: 700_000 }, { file: 'assets/index-a.js', bytes: 500_000 }]);
    const head = report(1_200_000, [{ file: 'assets/App-b.js', bytes: 640_000 }, { file: 'assets/icons-b.js', bytes: 60_000 }, { file: 'assets/index-b.js', bytes: 500_000 }]);
    const verdict = growth.growthVerdict(base, head, []);
    expect(verdict.deltaBytes).toBe(0);
    expect(verdict.kind).toBe('within');
  });

  it('prints raw and gzip beside each other', () => {
    const verdict = growth.growthVerdict(BASE, report(BASE.setBytes + 3_000), ['why']);
    expect(verdict.message).toMatch(/\+3\.00 kB raw \(\+1\.00 kB gzip\)/);
  });
});

describe('parseReasons', () => {
  it('reads every non-empty trailer value', () => {
    expect(growth.parseReasons('\nThe dock band draws on every cold open.\n\n\nA second one.\n')).toEqual([
      'The dock band draws on every cold open.', 'A second one.',
    ]);
    expect(growth.parseReasons('\n\n')).toEqual([]);
  });
});

describe('parseReport', () => {
  it('reads the report the budget plugin writes', () => {
    const text = JSON.stringify({
      version: 1,
      entry: { file: 'assets/index-a.js', bytes: 531_000 },
      firstPaint: { bytes: 1_233_000, gzipBytes: 402_000, budgetBytes: 1_400_000, chunks: [{ file: 'assets/index-a.js', bytes: 531_000 }] },
    });
    expect(growth.parseReport(text)).toEqual({
      entryBytes: 531_000, setBytes: 1_233_000, setGzipBytes: 402_000, budgetBytes: 1_400_000,
      chunks: [{ file: 'assets/index-a.js', bytes: 531_000 }],
    });
  });

  it('refuses a malformed report rather than measure from it', () => {
    expect(growth.parseReport('{}')).toBeNull();
    expect(growth.parseReport('not json')).toBeNull();
  });
});

describe('isFirstPaintInput', () => {
  it('counts the app, the SDK and the lockfile, and not tests', () => {
    expect(growth.isFirstPaintInput('crates/lucidos-app/src/App.tsx')).toBe(true);
    expect(growth.isFirstPaintInput('packages/lucidos-sdk/src/find.ts')).toBe(true);
    expect(growth.isFirstPaintInput('package-lock.json')).toBe(true);
    expect(growth.isFirstPaintInput('crates/lucidos-app/src/__tests__/x.test.ts')).toBe(false);
    expect(growth.isFirstPaintInput('crates/lucidos-app/src/store/a.test.tsx')).toBe(false);
    expect(growth.isFirstPaintInput('crates/lucidos-app/e2e/chat.spec.ts')).toBe(false);
    expect(growth.isFirstPaintInput('crates/lucidos-engine/src/main.rs')).toBe(false);
    expect(growth.isFirstPaintInput('crates/lucidos-app/src/lib.rs')).toBe(false);
    expect(growth.isFirstPaintInput('crates/lucidos-app/Cargo.toml')).toBe(false);
    expect(growth.isFirstPaintInput('system-widgets/sound-player/index.html')).toBe(true);
    expect(growth.isFirstPaintInput('docs/adr/0454-first-paint-growth-check.md')).toBe(false);
  });
});

describe('the report path', () => {
  it('is named the same in the plugin and the tool', () => {
    expect(growth.FIRST_PAINT_REPORT_FILE).toBe(FIRST_PAINT_REPORT_FILE);
  });
});

describe('the ruler', () => {
  it("builds the tree's own config with this checkout's budget plugin added in report mode", () => {
    const source = growth.rulerConfigSource('/tree/app/vite.config.ts', '/checkout/app/vite/chunkBudget.ts');
    expect(source).toContain('import base from "/tree/app/vite.config.ts";');
    expect(source).toContain('import { chunkBudget } from "/checkout/app/vite/chunkBudget.ts";');
    expect(source).toContain('plugins: [...(config.plugins ?? []), chunkBudget(false, true)]');
    expect(source).toContain("typeof base === 'function' ? await base(env) : base");
  });
});

// ── the baseline, over real git ─────────────────────────────────────────────

let sandbox: Sandbox;
const git = (...args: string[]) => sandbox.git(...args);

/** A commit whose app reports `size` bytes, through the stub build below. */
function commitSize(size: number, message: string) {
  sandbox.write('crates/lucidos-app/src/size.txt', String(size));
  git('add', '-A');
  git('commit', '-q', '-m', message);
  return git('rev-parse', 'HEAD');
}

/** Stands in for `vite build`: reads the archived tree's size file. A tree
 *  without one writes no report, as a base with nothing to build does. */
const stubBuild: Build = ({ appDir, reportPath }) => {
  const size = readIfPresent(join(appDir, 'src/size.txt'));
  if (size === null) return;
  writeAt(reportPath, JSON.stringify({
    version: 1,
    entry: { file: 'assets/index.js', bytes: 1 },
    firstPaint: { bytes: Number(size), gzipBytes: Math.round(Number(size) / 3), budgetBytes: FIRST_PAINT_BUDGET.kb * 1000, chunks: [{ file: 'assets/index.js', bytes: Number(size) }] },
  }));
};

function measureHeadFromWorktree() {
  const reportPath = join(tempDir('fp-head-'), 'report.json');
  stubBuild({ appDir: join(sandbox.root, 'crates/lucidos-app'), outDir: '', reportPath });
  return growth.parseReport(readIfPresent(reportPath) ?? '');
}

function check(extra: Partial<Parameters<typeof growth.runCheck>[0]> = {}) {
  return growth.runCheck({
    repoRoot: sandbox.root,
    baseRef: 'main',
    cacheDir: join(sandbox.root, '.lucidos/first-paint'),
    build: stubBuild,
    measureHead: measureHeadFromWorktree,
    ...extra,
  });
}

beforeEach(() => {
  sandbox = makeRepo();
});

afterEach(() => {
  sandbox.remove();
});

// Each case spawns a few dozen git processes, which outruns the default limit.
describe('runCheck against the merge base', { timeout: 30_000 }, () => {
  it('fails growth with no reason, and passes once a commit states one', () => {
    commitSize(1_000_000, 'base');
    git('checkout', '-q', '-b', 'feature');
    commitSize(1_000_000 + OVER, 'feat: grow');
    expect(check().exitCode).toBe(1);
    git('commit', '-q', '--allow-empty', '-m', `docs: say why\n\n${GROWTH_TRAILER}: The new card draws on a cold open.`);
    const result = check();
    expect(result.exitCode).toBe(0);
    expect(result.text).toContain('The new card draws on a cold open.');
  });

  it('passes a shrink and prints it negative', () => {
    commitSize(1_000_000, 'base');
    git('checkout', '-q', '-b', 'feature');
    commitSize(990_000, 'perf: trim');
    const result = check();
    expect(result.exitCode).toBe(0);
    expect(result.text).toContain('-10.00 kB');
  });

  it('builds nothing when no first-paint input changed', () => {
    commitSize(1_000_000, 'base');
    git('checkout', '-q', '-b', 'feature');
    sandbox.write('docs/note.md', 'x');
    git('add', '-A');
    git('commit', '-q', '-m', 'docs: note');
    let built = false;
    const result = check({ build: () => { built = true; }, measureHead: () => { built = true; return null; } });
    expect(built).toBe(false);
    expect(result.exitCode).toBe(0);
    expect(result.text).toContain('no first-paint input changed');
  });

  it('fails closed with no merge base, and says to merge main', () => {
    commitSize(1_000_000, 'base');
    git('checkout', '-q', '--orphan', 'unrelated');
    commitSize(1_000_000, 'unrelated root');
    const result = check();
    expect(result.exitCode).toBe(1);
    expect(result.text).toMatch(/no merge base/i);
    expect(result.text).toContain('merge main');
  });

  it('fails closed when the base ref does not exist', () => {
    commitSize(1_000_000, 'base');
    const result = check({ baseRef: 'no-such-branch' });
    expect(result.exitCode).toBe(1);
    expect(result.text).toMatch(/no merge base/i);
  });

  it('fails closed on a stale merge base that has nothing to build', () => {
    sandbox.write('README', 'old tree with no app');
    git('add', '-A');
    git('commit', '-q', '-m', 'an old base, before the app');
    git('checkout', '-q', '-b', 'feature');
    commitSize(1_000_000, 'feat: the app');
    const result = check();
    expect(result.exitCode).toBe(1);
    expect(result.text).toMatch(/wrote no first-paint report/);
    expect(result.text).toContain('merge main');
  });

  it('measures two parallel branches against their own base, however main moves', () => {
    const base = commitSize(1_000_000, 'base');
    git('checkout', '-q', '-b', 'a');
    commitSize(1_001_500, 'feat: a grows 1.5 kB');
    git('checkout', '-q', 'main');
    git('checkout', '-q', '-b', 'b');
    commitSize(1_001_000, 'feat: b grows 1 kB');
    // A lands, the way Apply lands a change: main fast-forwards to its tip.
    git('checkout', '-q', 'main');
    git('merge', '-q', '--ff-only', 'a');
    git('checkout', '-q', 'b');
    expect(growth.resolveMergeBase(sandbox.root, 'main')).toBe(base);
    const result = check();
    expect(result.exitCode).toBe(0);
    expect(result.text).toContain('+1.00 kB');
  });

  it('installs a base its own dependencies when its lockfile differs, and reuses ours when it matches', () => {
    sandbox.write('package-lock.json', '{"lockfileVersion":3,"v":"old"}');
    commitSize(1_000_000, 'base');
    git('checkout', '-q', '-b', 'feature');
    const installed: string[] = [];
    const install = (tree: string) => { installed.push(readIfPresent(join(tree, 'package-lock.json')) ?? ''); };

    commitSize(1_000_500, 'feat: same dependencies');
    expect(check({ install }).exitCode).toBe(0);
    expect(installed).toEqual([]);

    sandbox.write('package-lock.json', '{"lockfileVersion":3,"v":"new"}');
    commitSize(1_000_500, 'chore(deps): bump');
    expect(check({ install, cacheDir: join(sandbox.root, '.lucidos/fresh') }).exitCode).toBe(0);
    expect(installed).toEqual(['{"lockfileVersion":3,"v":"old"}']);
  });

  it('fails closed when the base install fails, naming the base', () => {
    sandbox.write('package-lock.json', '{"v":"old"}');
    commitSize(1_000_000, 'base');
    git('checkout', '-q', '-b', 'feature');
    sandbox.write('package-lock.json', '{"v":"new"}');
    commitSize(1_000_500, 'chore(deps): bump');
    const result = check({ install: () => { throw new Error('offline'); } });
    expect(result.exitCode).toBe(1);
    expect(result.text).toMatch(/installing the dependencies of [0-9a-f]{7} failed: offline/);
  });

  describe('the registry a base installs from', () => {
    const USER_REGISTRY = 'https://registry.invalid/';

    /** Branch off a base and bump the lockfile, so the base needs its own install.
     *  The user config and npm_config_registry name a registry outside the repo,
     *  as a contributor's own setup can. Returns what npm resolves in the base tree. */
    function baseRegistry(onFeature: () => void) {
      git('checkout', '-q', '-b', 'feature');
      onFeature();
      sandbox.write('package-lock.json', '{"v":"new"}');
      commitSize(1_000_500, 'chore(deps): bump');
      const home = tempDir('fp-home-');
      const userconfig = join(home, '.npmrc');
      writeAt(userconfig, `registry=${USER_REGISTRY}\n@example:registry=${USER_REGISTRY}\n`);
      const env = envWith({ HOME: home, npm_config_userconfig: userconfig, npm_config_registry: USER_REGISTRY });
      const seen: string[] = [];
      try {
        const result = check({
          install: (tree: string) => { seen.push(growth.treeNpm(tree, ['config', 'get', 'registry', '@example:registry'], env)); },
        });
        expect(result.text).not.toContain('failed');
      } finally {
        removeDir(home);
      }
      return seen;
    }

    it("uses the base commit's own .npmrc, whatever the user config says", () => {
      sandbox.write('.npmrc', 'registry=https://base-registry.invalid/\n');
      sandbox.write('package-lock.json', '{"v":"old"}');
      commitSize(1_000_000, 'base');
      const seen = baseRegistry(() => sandbox.write('.npmrc', 'registry=https://head-registry.invalid/\n'));
      expect(seen).toEqual(['registry=https://base-registry.invalid/\n@example:registry=undefined']);
    });

    it("falls back to this checkout's .npmrc when the base pins none", () => {
      sandbox.write('package-lock.json', '{"v":"old"}');
      commitSize(1_000_000, 'base');
      const seen = baseRegistry(() => sandbox.write('.npmrc', 'registry=https://repo-registry.invalid/\n'));
      expect(seen).toEqual(['registry=https://repo-registry.invalid/\n@example:registry=undefined']);
    });
  });

  it('builds the base from an archive and leaves the worktree untouched', () => {
    commitSize(1_000_000, 'base');
    git('checkout', '-q', '-b', 'feature');
    commitSize(1_000_500, 'feat: grow a little');
    sandbox.write('crates/lucidos-app/src/size.txt', '1000700');
    const before = git('status', '--porcelain');
    check();
    expect(git('status', '--porcelain')).toBe(before);
    expect(sandbox.read('crates/lucidos-app/src/size.txt')).toBe('1000700');
    expect(git('rev-parse', '--abbrev-ref', 'HEAD')).toBe('feature');
  });

  it('caches the base measurement by commit', () => {
    commitSize(1_000_000, 'base');
    git('checkout', '-q', '-b', 'feature');
    commitSize(1_000_500, 'feat: grow a little');
    let builds = 0;
    const counting: Build = (args) => { builds += 1; stubBuild(args); };
    check({ build: counting });
    check({ build: counting });
    expect(builds).toBe(1);
  });
});

// ── the nightly ranking ─────────────────────────────────────────────────────

describe('runReport over main\'s reflog', { timeout: 30_000 }, () => {
  /** Land a change the way Apply does: main fast-forwards to the branch tip. */
  function land(branch: string, size: number, subject: string, reason?: string) {
    git('checkout', '-q', '-b', branch);
    commitSize(size, subject);
    if (reason) git('commit', '-q', '--allow-empty', '-m', 'chore: say why', '-m', `${GROWTH_TRAILER}: ${reason}`);
    git('checkout', '-q', 'main');
    git('merge', '-q', '--ff-only', branch);
  }

  it('ranks each landed change with its reason, and shows a shrink negative', () => {
    commitSize(1_000_000, 'base');
    land('dock', 1_006_000, 'feat: the dock band', 'Docked widgets draw on a cold open.');
    land('quiet', 1_009_500, 'fix: a quiet adder');
    land('trim', 1_004_000, 'perf: trim the shell');
    const result = growth.runReport({
      repoRoot: sandbox.root, hours: 1, cacheDir: join(sandbox.root, '.lucidos/first-paint'), build: stubBuild,
    });
    expect(result.exitCode).toBe(0);
    const lines = result.text.split('\n');
    const dock = lines.findIndex((l: string) => l.includes('feat: the dock band'));
    const quiet = lines.findIndex((l: string) => l.includes('fix: a quiet adder'));
    expect(lines[dock]).toContain('+6.00 kB raw');
    expect(lines[dock]).toContain('Docked widgets draw on a cold open.');
    expect(lines[quiet]).toContain('+3.50 kB raw');
    expect(lines[quiet]).toContain('no reason given');
    expect(dock).toBeLessThan(quiet);
    expect(result.text).toMatch(/-5\.50 kB raw .*perf: trim the shell/);
    expect(result.text).toContain('net over 3 changes');
  });

  it('names a change it could not measure rather than skipping it quietly', () => {
    sandbox.write('README', 'no app yet');
    git('add', '-A');
    git('commit', '-q', '-m', 'an empty start');
    land('first', 1_000_000, 'feat: the app arrives');
    const result = growth.runReport({
      repoRoot: sandbox.root, hours: 1, cacheDir: join(sandbox.root, '.lucidos/first-paint'), build: stubBuild,
    });
    expect(result.text).toContain('Not measured');
    expect(result.text).toContain('feat: the app arrives');
  });
});


describe('changesFromReflog', () => {
  it('turns main moves, newest first, into changes in the order they landed', () => {
    expect(growth.changesFromReflog('ccc\nbbb\nbbb\naaa\n')).toEqual([
      { from: 'aaa', to: 'bbb' },
      { from: 'bbb', to: 'ccc' },
    ]);
    expect(growth.changesFromReflog('aaa\n')).toEqual([]);
  });
});

describe('formatReport', () => {
  it('ranks the biggest adders first, with their reasons, and shows shrinks negative', () => {
    const text = growth.formatReport({
      since: '24 hours ago',
      latest: report(1_245_000),
      changes: [
        { to: '1111111aaa', subject: 'feat: small', deltaBytes: 400, deltaGzipBytes: 130, reasons: [] },
        { to: '2222222bbb', subject: 'feat: the dock band', deltaBytes: 6_500, deltaGzipBytes: 2_100, reasons: ['Docked widgets draw on a cold open.'] },
        { to: '3333333ccc', subject: 'perf: trim the shell', deltaBytes: -9_000, deltaGzipBytes: -3_000, reasons: [] },
        { to: '4444444ddd', subject: 'fix: a quiet one', deltaBytes: 3_100, deltaGzipBytes: 900, reasons: [] },
      ],
    });
    const lines = text.split('\n');
    const dock = lines.findIndex((l: string) => l.includes('the dock band'));
    const quiet = lines.findIndex((l: string) => l.includes('a quiet one'));
    expect(dock).toBeGreaterThan(-1);
    expect(dock).toBeLessThan(quiet);
    expect(lines[dock]).toContain('+6.50 kB');
    expect(lines[dock]).toContain('Docked widgets draw on a cold open.');
    expect(lines[quiet]).toContain('no reason given');
    const small = lines.find((l: string) => l.includes('feat: small'));
    expect(small).not.toContain('no reason given');
    expect(text).not.toMatch(/[0-9a-f]{7}/);
    expect(text).toContain('-9.00 kB');
    expect(text).toContain('+1.00 kB raw');
    expect(text).toContain('1245.00 kB');
  });

  it('says so when main did not move', () => {
    expect(growth.formatReport({ since: '24 hours ago', latest: null, changes: [] })).toMatch(/no change/i);
  });
});

describe('prepareGenerated', () => {
  function writeDeep(path: string, text: string) {
    mkdirSync(dirname(path), { recursive: true });
    writeAt(path, text);
  }

  /** A repo whose codegen.sh answers `--cache-dir` from a table, by revision. */
  function repoWithCache(renderings: Record<string, string>): string {
    const root = tempDir('first-paint-codegen-');
    const cases = Object.entries(renderings)
      .map(([rev, dir]) => `  "${rev}") echo "${dir}" ;;`)
      .join('\n');
    writeDeep(join(root, 'scripts/codegen.sh'), `case "\${2:-tree}" in\n${cases}\n  *) exit 1 ;;\nesac\n`);
    return root;
  }

  function rendering(text: string): string {
    const dir = tempDir('first-paint-rendering-');
    writeDeep(join(dir, 'crates/lucidos-app/src/generated/a.ts'), text);
    return dir;
  }

  it("copies the commit's rendering into the extracted tree", () => {
    const tree = tempDir('first-paint-tree-');
    growth.prepareGenerated({ repoRoot: repoWithCache({ abc1234: rendering('base') }), sha: 'abc1234', tree });
    expect(readIfPresent(join(tree, 'crates/lucidos-app/src/generated/a.ts'))).toBe('base');
  });

  it("falls back to this tree's rendering when the cache holds none for the commit", () => {
    const tree = tempDir('first-paint-tree-');
    growth.prepareGenerated({ repoRoot: repoWithCache({ tree: rendering('head') }), sha: 'abc1234', tree });
    expect(readIfPresent(join(tree, 'crates/lucidos-app/src/generated/a.ts'))).toBe('head');
  });

  it('keeps a generated file the archive already holds', () => {
    const tree = tempDir('first-paint-tree-');
    writeDeep(join(tree, 'crates/lucidos-app/src/generated/a.ts'), 'committed');
    growth.prepareGenerated({ repoRoot: repoWithCache({ abc1234: rendering('base') }), sha: 'abc1234', tree });
    expect(readIfPresent(join(tree, 'crates/lucidos-app/src/generated/a.ts'))).toBe('committed');
  });

  it('fails when nothing is cached, rather than building without generated files', () => {
    expect(() => growth.prepareGenerated({ repoRoot: repoWithCache({}), sha: 'abc1234', tree: tempDir('first-paint-tree-') }))
      .toThrow(/codegen cache/);
  });
});
