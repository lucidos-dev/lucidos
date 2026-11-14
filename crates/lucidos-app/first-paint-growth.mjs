#!/usr/bin/env node
// What a change adds to the first-paint set (ADR 0436), and why (ADR 0454).
//
//   node first-paint-growth.mjs check [--base <ref>]
//   node first-paint-growth.mjs report [--hours <n>] [--top <n>]
//
// `check` builds the working tree and the change's merge base with `<ref>`
// (default `main`), and compares the two sets. Growth over the threshold needs
// a `First-paint-growth:` trailer on a commit of the change, or it exits 1.
// `report` ranks the changes main took in the last hours, read from main's
// reflog: Apply fast-forwards main, so each reflog entry is one change.
//
// Every number comes from the report `vite/chunkBudget.ts` emits. Both trees
// are measured with THIS checkout's copy, through a wrapper config, so the two
// sides share one ruler. Nothing here measures a chunk itself. The base builds
// from `git archive` output in a temp directory: no tracked file is touched.

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Mirrors `vite/chunkBudget.ts`, which this JS cannot import.
// `first-paint-growth.test.ts` fails when the two drift apart.
export const FIRST_PAINT_REPORT_FILE = 'first-paint-report.json';

/** The one ruler: this checkout's budget plugin, applied to both trees. */
const RULER = fileURLToPath(new URL('./vite/chunkBudget.ts', import.meta.url));

/** Growth past this, in raw bytes, needs a stated reason (ADR 0454). */
export const GROWTH_THRESHOLD_BYTES = 2000;

/** The commit trailer that states the reason. */
export const GROWTH_TRAILER = 'First-paint-growth';

/** Everything a build reads, relative to the repo root: what a base archive
 *  holds, and the roots of a first-paint input. */
export const ARCHIVE_PATHS = ['crates/lucidos-app', 'packages', 'package.json', 'package-lock.json', 'system-widgets'];

/** The repo's npm config, which pins the registry. A base archive carries it
 *  too, but it is no first-paint input: the lockfile pins the bytes. */
const NPM_CONFIG = '.npmrc';

const APP_DIR = 'crates/lucidos-app';

// ── pure helpers, exported for tests ────────────────────────────────────────

/** Signed kB of 1000 bytes, as Vite prints them. */
export function signedKb(bytes) {
  return `${bytes < 0 ? '-' : '+'}${(Math.abs(bytes) / 1000).toFixed(2)} kB`;
}

function kb(bytes) {
  return `${(bytes / 1000).toFixed(2)} kB`;
}

/** Whether a changed path can change the first-paint set. Broad on purpose:
 *  a path counted wrongly costs one build, a path missed costs the check. */
export function isFirstPaintInput(path) {
  if (/(^|\/)__tests__\//.test(path) || /\.(test|spec)\.[cm]?[jt]sx?$/.test(path)) return false;
  if (path.startsWith(`${APP_DIR}/e2e/`) || /(\.rs|(^|\/)Cargo\.toml)$/.test(path)) return false;
  return ARCHIVE_PATHS.some((root) => path === root || path.startsWith(`${root}/`));
}

/** The report `chunkBudget.ts` emits, or `null` when it is not one. */
export function parseReport(text) {
  try {
    const { firstPaint, entry } = JSON.parse(text);
    if (!firstPaint || !Number.isFinite(firstPaint.bytes) || !Number.isFinite(firstPaint.gzipBytes)) return null;
    return {
      entryBytes: entry?.bytes ?? 0,
      setBytes: firstPaint.bytes,
      setGzipBytes: firstPaint.gzipBytes,
      budgetBytes: firstPaint.budgetBytes,
      chunks: firstPaint.chunks ?? [],
    };
  } catch {
    return null;
  }
}

/** The trailer values `git log --format=%(trailers:...,valueonly)` printed. */
export function parseReasons(output) {
  return output.split('\n').map((line) => line.trim()).filter(Boolean);
}

function growthLine(base, head) {
  const delta = head.setBytes - base.setBytes;
  const gzip = head.setGzipBytes - base.setGzipBytes;
  return `${signedKb(delta)} raw (${signedKb(gzip)} gzip), from ${kb(base.setBytes)} to ${kb(head.setBytes)}`;
}

/**
 * Judge one change's growth. The whole set's delta is judged, so moving bytes
 * between first-paint chunks is no growth, and moving them behind a dynamic
 * import is a shrink.
 */
export function growthVerdict(base, head, reasons) {
  const deltaBytes = head.setBytes - base.setBytes;
  const deltaGzipBytes = head.setGzipBytes - base.setGzipBytes;
  const line = `First-paint growth: ${growthLine(base, head)}.`;
  const threshold = kb(GROWTH_THRESHOLD_BYTES);
  if (deltaBytes < 0) return { kind: 'shrink', deltaBytes, deltaGzipBytes, message: `${line} A shrink.` };
  if (deltaBytes <= GROWTH_THRESHOLD_BYTES) {
    return { kind: 'within', deltaBytes, deltaGzipBytes, message: `${line} Within the ${threshold} threshold.` };
  }
  if (reasons.length > 0) {
    const stated = reasons.map((r) => `  - ${r}`).join('\n');
    return {
      kind: 'reasoned', deltaBytes, deltaGzipBytes,
      message: `${line} Over the ${threshold} threshold, with this reason:\n${stated}`,
    };
  }
  return {
    kind: 'fail', deltaBytes, deltaGzipBytes,
    message: `${line} Over the ${threshold} threshold, with no reason given.\n`
      + 'Move code the first frame does not draw behind a dynamic import, or say why\n'
      + 'the first frame needs it, in a trailer on a commit of this change:\n\n'
      + `  ${GROWTH_TRAILER}: <why a cold open must load it>\n\nSee ADR 0454.`,
  };
}

/** Main's moves, newest first, as changes in the order they landed. */
export function changesFromReflog(output) {
  const tips = output.split('\n').map((l) => l.trim()).filter(Boolean).reverse();
  const changes = [];
  for (let i = 1; i < tips.length; i += 1) {
    if (tips[i] !== tips[i - 1]) changes.push({ from: tips[i - 1], to: tips[i] });
  }
  return changes;
}

const TOP_ADDERS = 5;
const TOP_SHRINKS = 3;

/** One change, named by its subject. Past the threshold, "no reason given"
 *  marks a change that skipped the gate. */
function changeLine(c) {
  let why = '';
  if (c.reasons.length > 0) why = `: ${c.reasons.join(' / ')}`;
  else if (c.deltaBytes > GROWTH_THRESHOLD_BYTES) why = ': no reason given';
  return `- ${signedKb(c.deltaBytes)} raw (${signedKb(c.deltaGzipBytes)} gzip) ${c.subject}${why}`;
}

/** The morning notice's first-paint lines. `changes` holds only measured ones. */
export function formatReport({ since, latest, changes, top = TOP_ADDERS }) {
  if (changes.length === 0) return `First-paint growth: no change to main touched the first-paint set in the ${since}.`;
  const net = changes.reduce((sum, c) => sum + c.deltaBytes, 0);
  const netGzip = changes.reduce((sum, c) => sum + c.deltaGzipBytes, 0);
  const adders = changes.filter((c) => c.deltaBytes > 0).sort((a, b) => b.deltaBytes - a.deltaBytes).slice(0, top);
  const shrinks = changes.filter((c) => c.deltaBytes < 0).sort((a, b) => a.deltaBytes - b.deltaBytes).slice(0, TOP_SHRINKS);
  const lines = [
    `First-paint growth, ${since}: ${signedKb(net)} raw (${signedKb(netGzip)} gzip) net over ${changes.length} changes.`
      + (latest ? ` The set is ${kb(latest.setBytes)}, budget ${kb(latest.budgetBytes)}.` : ''),
  ];
  if (adders.length > 0) lines.push('Biggest adders:', ...adders.map(changeLine));
  if (shrinks.length > 0) lines.push('Shrinks:', ...shrinks.map(changeLine));
  return lines.join('\n');
}

// ── git and builds ──────────────────────────────────────────────────────────

function git(repoRoot, args) {
  const out = spawnSync('git', args, { cwd: repoRoot, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
  return { ok: out.status === 0, stdout: (out.stdout ?? '').trim() };
}

/** The merge base of HEAD with `baseRef`, or `null` when there is none. */
export function resolveMergeBase(repoRoot, baseRef) {
  const out = git(repoRoot, ['merge-base', baseRef, 'HEAD']);
  return out.ok && out.stdout ? out.stdout : null;
}

/** What a cached base result depends on besides its commit: the installed
 *  dependencies, and the ruler with the module it reads chunks through. */
function measureKey(repoRoot) {
  const hash = createHash('sha256');
  for (const path of [join(repoRoot, 'package-lock.json'), RULER, join(dirname(RULER), 'shellChunk.ts')]) {
    hash.update(existsSync(path) ? readFileSync(path) : 'missing');
  }
  return hash.digest('hex').slice(0, 12);
}

/** The wrapper config: `appDir`'s own config, plus the ruler in report mode. */
export function rulerConfigSource(appConfig, ruler = RULER) {
  return [
    `import base from ${JSON.stringify(appConfig)};`,
    `import { chunkBudget } from ${JSON.stringify(ruler)};`,
    'export default async (env) => {',
    "  const config = typeof base === 'function' ? await base(env) : base;",
    '  return { ...config, plugins: [...(config.plugins ?? []), chunkBudget(false, true)] };',
    '};',
    '',
  ].join('\n');
}

/** Run `vite build` in `appDir` through the wrapper config, into `outDir`. */
export function viteBuild({ appDir, outDir }) {
  const appConfig = join(appDir, 'vite.config.ts');
  if (!existsSync(appConfig)) throw new Error(`no vite.config.ts in ${appDir}`);
  const vitePackage = createRequire(join(appDir, 'package.json')).resolve('vite/package.json');
  // Vite loads the bundled config from beside the wrapper, so the wrapper's
  // directory needs the app's node_modules to resolve the config's imports.
  const configDir = mkdtempSync(join(tmpdir(), 'first-paint-config-'));
  try {
    symlinkSync(dirname(dirname(vitePackage)), join(configDir, 'node_modules'));
    const wrapper = join(configDir, 'first-paint.config.mjs');
    writeFileSync(wrapper, rulerConfigSource(appConfig));
    const env = { ...process.env };
    delete env.LUCIDOS_ATOMIC_DIST;
    delete env.LUCIDOS_DEV_BUILD_WATCH;
    const bin = resolve(dirname(vitePackage), 'bin/vite.js');
    const out = spawnSync(process.execPath, [bin, 'build', '--config', wrapper, '--outDir', outDir, '--emptyOutDir'], {
      cwd: appDir, env, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024,
    });
    if (out.status !== 0) {
      const tail = `${out.stdout ?? ''}\n${out.stderr ?? ''}`.trim().split('\n').slice(-12).join('\n');
      throw new Error(`vite build exited ${out.status}:\n${tail}`);
    }
  } finally {
    rmSync(configDir, { recursive: true, force: true });
  }
}

function measureInto(appDir, build) {
  const outDir = mkdtempSync(join(tmpdir(), 'first-paint-out-'));
  try {
    build({ appDir, outDir, reportPath: join(outDir, FIRST_PAINT_REPORT_FILE) });
    const path = join(outDir, FIRST_PAINT_REPORT_FILE);
    return existsSync(path) ? parseReport(readFileSync(path, 'utf8')) : null;
  } finally {
    rmSync(outDir, { recursive: true, force: true });
  }
}

/**
 * Give an extracted commit the generated files its build reads. They are build
 * outputs, so the archive may lack them. The commit's rendering comes from the
 * codegen cache, or with none cached, this tree's. A file the archive already
 * holds stays.
 */
export function prepareGenerated({ repoRoot, sha, tree }) {
  const codegen = join(repoRoot, 'scripts/codegen.sh');
  if (existsSync(codegen)) {
    const cacheDir = (...rev) => spawnSync('bash', [codegen, '--cache-dir', ...rev], { cwd: repoRoot, encoding: 'utf8' });
    let found = cacheDir(sha);
    if (found.status !== 0) found = cacheDir();
    if (found.status !== 0) throw new Error(`no rendering of ${sha.slice(0, 7)} or of this tree in the codegen cache. Run: ./scripts/codegen.sh`);
    cpSync(found.stdout.trim(), tree, { recursive: true, force: false });
  }
  const sdk = join(tree, 'packages/lucidos-sdk');
  if (!existsSync(join(sdk, 'build.mjs'))) return;
  const out = spawnSync(process.execPath, ['build.mjs'], { cwd: sdk, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
  if (out.status !== 0) {
    const tail = `${out.stdout ?? ''}\n${out.stderr ?? ''}`.trim().split('\n').slice(-12).join('\n');
    throw new Error(`the SDK build exited ${out.status}:\n${tail}`);
  }
}

function runNpm(cwd, args, env) {
  const out = spawnSync('npm', args, { cwd, env, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
  if (out.status !== 0) {
    const tail = `${out.stdout ?? ''}\n${out.stderr ?? ''}`.trim().split('\n').slice(-12).join('\n');
    throw new Error(`npm ${args[0]} exited ${out.status}:\n${tail}`);
  }
  return (out.stdout ?? '').trim();
}

/**
 * Run npm in `tree` with the tree's `.npmrc` as its only config, and return
 * its output. User and global config files are empty, and no `npm_config_*`
 * variable gets through, so a contributor's registry cannot reach the install.
 * The cache stays the ambient one, for `--prefer-offline`.
 */
export function treeNpm(tree, args, env = process.env) {
  const cache = runNpm(tree, ['config', 'get', 'cache'], env);
  const configDir = mkdtempSync(join(tmpdir(), 'first-paint-npmrc-'));
  try {
    const userconfig = join(configDir, 'user.npmrc');
    const globalconfig = join(configDir, 'global.npmrc');
    writeFileSync(userconfig, '');
    writeFileSync(globalconfig, '');
    const repoOnly = Object.fromEntries(Object.entries(env).filter(([key]) => !/^npm_config_/i.test(key)));
    return runNpm(tree, [...args, '--userconfig', userconfig, '--globalconfig', globalconfig, '--cache', cache], repoOnly);
  } finally {
    rmSync(configDir, { recursive: true, force: true });
  }
}

/** Install `tree`'s own dependencies from its own lockfile, so a base is built
 *  with the versions it pinned. */
export function npmCi(tree) {
  treeNpm(tree, ['ci', '--ignore-scripts', '--prefer-offline', '--no-audit', '--no-fund']);
}

function readBytes(path) {
  return existsSync(path) ? readFileSync(path) : null;
}

/** Build the working tree and read its report. */
export function measureWorktree(repoRoot, build = viteBuild) {
  return measureInto(join(repoRoot, APP_DIR), build);
}

/**
 * Build a commit from its archive and read its report, cached by commit and
 * lockfile. A commit whose lockfile differs from this checkout's gets its own
 * install, so a dependency change measures. `{ error }` when the commit has
 * nothing to build, its install or build fails, or it writes no report.
 */
export function measureCommit({ repoRoot, sha, cacheDir, build = viteBuild, install = npmCi }) {
  const cached = join(cacheDir, `${sha}-${measureKey(repoRoot)}.json`);
  if (existsSync(cached)) {
    const report = parseReport(readFileSync(cached, 'utf8'));
    if (report) return { report };
  }
  const noReport = {
    error: `${sha.slice(0, 7)} wrote no first-paint report: its tree has no app to build.`,
  };
  const paths = [...ARCHIVE_PATHS, NPM_CONFIG].filter((p) => git(repoRoot, ['cat-file', '-e', `${sha}:${p}`]).ok);
  if (!paths.includes(APP_DIR)) return noReport;
  const tree = mkdtempSync(join(tmpdir(), 'first-paint-tree-'));
  try {
    const archive = spawnSync('git', ['archive', '--format=tar', sha, ...paths], { cwd: repoRoot, maxBuffer: 1024 * 1024 * 1024 });
    if (archive.status !== 0) return { error: `git archive of ${sha.slice(0, 7)} failed.` };
    const untar = spawnSync('tar', ['-x', '-C', tree], { input: archive.stdout });
    if (untar.status !== 0) return { error: `extracting ${sha.slice(0, 7)} failed.` };
    const ownLock = readBytes(join(tree, 'package-lock.json'));
    const repoLock = readBytes(join(repoRoot, 'package-lock.json'));
    if (ownLock && !(repoLock && ownLock.equals(repoLock))) {
      // A base that pinned no registry installs from ours, never the user's.
      if (!existsSync(join(tree, NPM_CONFIG)) && existsSync(join(repoRoot, NPM_CONFIG))) {
        copyFileSync(join(repoRoot, NPM_CONFIG), join(tree, NPM_CONFIG));
      }
      try {
        install(tree);
      } catch (err) {
        return { error: `installing the dependencies of ${sha.slice(0, 7)} failed: ${err.message}` };
      }
    } else if (existsSync(join(repoRoot, 'node_modules'))) {
      symlinkSync(join(repoRoot, 'node_modules'), join(tree, 'node_modules'));
    }
    try {
      prepareGenerated({ repoRoot, sha, tree });
    } catch (err) {
      return { error: `preparing the generated files of ${sha.slice(0, 7)} failed: ${err.message}` };
    }
    let report;
    try {
      report = measureInto(join(tree, APP_DIR), build);
    } catch (err) {
      return { error: `the build of ${sha.slice(0, 7)} failed: ${err.message}` };
    }
    if (!report) return noReport;
    mkdirSync(cacheDir, { recursive: true });
    writeFileSync(cached, JSON.stringify({
      version: 1,
      entry: { bytes: report.entryBytes },
      firstPaint: {
        bytes: report.setBytes, gzipBytes: report.setGzipBytes, budgetBytes: report.budgetBytes, chunks: report.chunks,
      },
    }));
    return { report };
  } finally {
    rmSync(tree, { recursive: true, force: true });
  }
}

function changedPaths(repoRoot, base) {
  const tracked = git(repoRoot, ['diff', '--name-only', base]).stdout;
  const untracked = git(repoRoot, ['ls-files', '--others', '--exclude-standard']).stdout;
  return `${tracked}\n${untracked}`.split('\n').filter(Boolean);
}

function reasonsSince(repoRoot, base) {
  return parseReasons(git(repoRoot, ['log', `--format=%(trailers:key=${GROWTH_TRAILER},valueonly)`, `${base}..HEAD`]).stdout);
}

const MERGE_MAIN = 'Please merge main and run again.';

/** The `check` command. `exitCode` 0 passes; `text` says what was found. */
export function runCheck({
  repoRoot, baseRef = 'main', cacheDir = join(repoRoot, '.lucidos/first-paint'), build = viteBuild, install = npmCi,
  measureHead = () => measureWorktree(repoRoot, build),
}) {
  const base = resolveMergeBase(repoRoot, baseRef);
  if (!base) {
    return { exitCode: 1, text: `First-paint growth: no merge base with ${baseRef}, so no baseline. ${MERGE_MAIN}` };
  }
  if (!changedPaths(repoRoot, base).some(isFirstPaintInput)) {
    return { exitCode: 0, text: `First-paint growth: no first-paint input changed since the merge base ${base.slice(0, 7)}.` };
  }
  const measured = measureCommit({ repoRoot, sha: base, cacheDir, build, install });
  if (measured.error) {
    return { exitCode: 1, text: `First-paint growth: no baseline. The merge base ${measured.error} ${MERGE_MAIN}` };
  }
  let head;
  try {
    head = measureHead();
  } catch (err) {
    return { exitCode: 1, text: `First-paint growth: this tree's build failed.\n${err.message}` };
  }
  if (!head) return { exitCode: 1, text: 'First-paint growth: this tree wrote no first-paint report.' };
  const verdict = growthVerdict(measured.report, head, reasonsSince(repoRoot, base));
  return {
    exitCode: verdict.kind === 'fail' ? 1 : 0,
    text: `Against the merge base ${base.slice(0, 7)} with ${baseRef}.\n${verdict.message}`,
  };
}

/** Main's moves in the last `hours`, plus the tip before them, newest first. */
function reflogTips(repoRoot, ref, hours) {
  const since = Date.now() / 1000 - hours * 3600;
  const out = git(repoRoot, ['reflog', 'show', '--date=unix', '--format=%H %gd', `refs/heads/${ref}`]).stdout;
  const tips = [];
  for (const line of out.split('\n')) {
    const [sha, selector] = line.split(' ');
    const at = Number(/@\{(\d+)\}/.exec(selector ?? '')?.[1]);
    if (!sha || !Number.isFinite(at)) continue;
    tips.push(sha);
    if (at < since) break;
  }
  return tips;
}

/** The subject that names a change: its newest feature, fix or perf commit. */
function changeSubject(repoRoot, from, to) {
  const subjects = git(repoRoot, ['log', '--no-merges', '--format=%s', `${from}..${to}`]).stdout.split('\n').filter(Boolean);
  return subjects.find((s) => /^(feat|fix|perf|refactor)(\(|:)/.test(s)) ?? subjects[0] ?? '(a merge with no commits of its own)';
}

/** The `report` command, for the nightly. */
export function runReport({
  repoRoot, ref = 'main', hours = 24, top = TOP_ADDERS, cacheDir = join(repoRoot, '.lucidos/first-paint'), build = viteBuild,
  install = npmCi,
}) {
  const tips = reflogTips(repoRoot, ref, hours);
  if (tips.length === 0) return { exitCode: 0, text: `First-paint growth: ${ref} has no reflog here, so no ranking.` };
  const changes = [];
  const failures = [];
  for (const { from, to } of changesFromReflog(tips.join('\n'))) {
    const touched = git(repoRoot, ['diff', '--name-only', from, to]).stdout.split('\n').some(isFirstPaintInput);
    if (!touched) continue;
    const before = measureCommit({ repoRoot, sha: from, cacheDir, build, install });
    const after = measureCommit({ repoRoot, sha: to, cacheDir, build, install });
    if (before.error || after.error) {
      failures.push(`${changeSubject(repoRoot, from, to)}: ${before.error ?? after.error}`);
      continue;
    }
    changes.push({
      to,
      subject: changeSubject(repoRoot, from, to),
      deltaBytes: after.report.setBytes - before.report.setBytes,
      deltaGzipBytes: after.report.setGzipBytes - before.report.setGzipBytes,
      reasons: parseReasons(git(repoRoot, ['log', `--format=%(trailers:key=${GROWTH_TRAILER},valueonly)`, `${from}..${to}`]).stdout),
    });
  }
  const latest = measureCommit({ repoRoot, sha: tips[0], cacheDir, build, install }).report ?? null;
  const text = formatReport({ since: `last ${hours} hours`, latest, changes, top });
  const unmeasured = failures.length > 0 ? `\nNot measured:\n${failures.map((f) => `- ${f}`).join('\n')}` : '';
  return { exitCode: 0, text: text + unmeasured };
}

// ── the command line ────────────────────────────────────────────────────────

function option(args, name, fallback) {
  const at = args.indexOf(name);
  return at === -1 ? fallback : args[at + 1];
}

function main(argv) {
  const [command, ...args] = argv;
  const repoRoot = git(process.cwd(), ['rev-parse', '--show-toplevel']).stdout;
  let result;
  if (command === 'check') {
    result = runCheck({ repoRoot, baseRef: option(args, '--base', 'main') });
  } else if (command === 'report') {
    result = runReport({ repoRoot, hours: Number(option(args, '--hours', '24')), top: Number(option(args, '--top', String(TOP_ADDERS))) });
  } else {
    result = { exitCode: 2, text: 'usage: first-paint-growth.mjs check [--base <ref>] | report [--hours <n>] [--top <n>]' };
  }
  console.log(result.text);
  process.exitCode = result.exitCode;
}

const invokedDirectly = process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invokedDirectly) main(process.argv.slice(2));
