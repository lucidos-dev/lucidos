import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import { test } from 'node:test';
import { appMark, buildMeasures, hostFingerprint, idleStatus, labResult, verifyRelease } from './lab.mjs';

test('the step boundary stops after an ordinary build queues', () => {
  const idle = 'build slots: 0/3 held, capacity from capacity file\npool: /slots\n  slot 0  free\n';
  assert.equal(idleStatus(idle), true);
  assert.equal(idleStatus(idle + 'at least one build is waiting for a slot\n'), false);
  assert.equal(idleStatus(idle + 'a priority build is waiting: the next free slot goes to it\n'), false);
  assert.equal(idleStatus(idle.replace('0/3', '1/3')), false);
  assert.throws(() => idleStatus('unexpected output'), /status/);
});

test('release verification requires the real class field and the same slot request', () => {
  const request = randomUUID();
  const event = { event_type: 'BuildSlotReleased', payload: { request_id: request, class: 'background', exit_code: 0 } };
  assert.doesNotThrow(() => verifyRelease([event], request));
  assert.throws(() => verifyRelease([event], randomUUID()), /slot release/);
  assert.throws(() => verifyRelease([{ ...event, payload: { ...event.payload, class: 'ordinary' } }], request), /background/);
});

test('a result retains its host and every raw sample with a median', () => {
  const host = hostFingerprint('rustc version');
  assert.ok(host.os && host.architecture && host.cores > 0 && host.memory_bytes > 0);
  assert.equal(host.toolchain, 'rustc version');
  assert.match(host.fingerprint, /^[0-9a-f]{64}$/);
  const result = labResult({ host, measurements: { build: [3, 1, 2] }, unit: 'seconds' });
  assert.equal(result.host, host);
  assert.deepEqual(result.measurements.build, { unit: 'seconds', samples: [3, 1, 2], median: 2 });
  assert.throws(() => labResult({ host, measurements: { build: [1, 2] }, unit: 'seconds' }), /at least 3/);
  assert.throws(() => labResult({ host, measurements: { build: [0, -1, 3] }, unit: 'seconds' }), /finite/);
});

test('build steps match by name across repeated benchmark passes', () => {
  const rows = [1, 2, 3].map(n => `build\t${n}\t9\t?\t?\nclippy\t${n * 2}\t9\t?\t?\n`);
  assert.deepEqual(buildMeasures(rows), { build: [1, 2, 3], clippy: [2, 4, 6] });
  assert.throws(() => buildMeasures([...rows, 'build\t1\t3\t?\t?\n']), /steps|jobs/);
});

test('app journeys read the original mark for the intended action', () => {
  const sample = (message, data) => ({ category: 'perf', message, data });
  assert.equal(appMark('cold-boot', sample('boot', { deepLink: false, splashLiftedMs: 42 })), 42);
  assert.equal(appMark('notification-deep-link', sample('boot', { deepLink: true, landedMs: 93 })), 93);
  assert.equal(appMark('notification-deep-link', sample('boot', { deepLink: true, splashLiftedMs: 42 })), undefined);
  assert.equal(appMark('thread-navigation', sample('thread-render', { threadId: 'other', renderMs: 3 }), 'target'), undefined);
  assert.equal(appMark('thread-navigation', sample('thread-render', { threadId: 'target', renderMs: 3 }), 'target'), 3);
});

function fakeLab(t) {
  mkdirSync('target', { recursive: true });
  const dir = mkdtempSync(resolve('target/lab-test.'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  symlinkSync(fileURLToPath(new URL('.', import.meta.url)), join(dir, 'scoreboard'), 'dir');
  writeFileSync(join(dir, 'with-build-slot.sh'), `#!/usr/bin/env bash
[ "$LUCIDOS_BUILD_SLOT_CLASS" = background ] || exit 1
export LUCIDOS_BUILD_SLOT_GRANTED_CLASS=background LUCIDOS_BUILD_SLOT_REQUEST_ID=fixture-request
shift 3
exec "$@"
`, { mode: 0o755 });
  writeFileSync(join(dir, 'lucidos'), `#!/usr/bin/env bash
case "$1 $2" in
  "build-slot --status")
    echo "build slots: 0/3 held, capacity from capacity file"
    if [ -e "$LAB_OUT/started" ]; then echo "at least one build is waiting for a slot"; fi
    ;;
  "events query")
    printf '%s\n' '[{"event_type":"BuildSlotReleased","payload":{"request_id":"fixture-request","class":"background","exit_code":0}}]'
    ;;
  "events emit") printf '%s' "$7" > "$LAB_OUT/event.json" ;;
  *) exit 2 ;;
esac
`, { mode: 0o755 });
  const env = {
    ...process.env,
    LUCIDOS_BUILD_SLOT_HELD: '',
    PATH: `${dir}:${process.env.PATH}`,
  };
  return { dir, env };
}

test('the runner records deferral and does not start a second step when work queues', t => {
  const { dir, env } = fakeLab(t);
  const result = spawnSync('bash', ['-c', `
    source "$1"
    lab_step first touch "$LAB_OUT/started" || exit $?
    lab_step second touch "$LAB_OUT/started-second"
  `, '_', fileURLToPath(new URL('../lib/lab_slot.sh', import.meta.url))], {
    env: {
      ...env,
      SCRIPT_DIR: dir,
      LAB_OUT: dir,
      LAB_KIND: 'build',
      LAB_TOOLCHAIN: 'rustc fixture',
      LAB_RUN: 'fixture-run',
    },
    encoding: 'utf8',
  });
  assert.equal(result.status, 75, result.stdout + result.stderr);
  assert.throws(() => readFileSync(join(dir, 'started-second')), { code: 'ENOENT' });
  const event = JSON.parse(readFileSync(join(dir, 'event.json'), 'utf8'));
  assert.equal(event.status, 'deferred');
  assert.equal(event.next_step, 'second');
  assert.equal(event.host.toolchain, 'rustc fixture');
});

test('the app lab uses the caller for broker evidence and the disposable workspace for the driver', t => {
  const { dir, env } = fakeLab(t);
  const scripts = join(dir, 'scripts');
  const bin = join(dir, 'node_modules/.bin');
  mkdirSync(join(scripts, 'lib'), { recursive: true });
  mkdirSync(bin, { recursive: true });
  copyFileSync(new URL('../measure-app.sh', import.meta.url), join(scripts, 'measure-app.sh'));
  writeFileSync(join(scripts, 'lib/lab_slot.sh'), `lab_idle() { :; }
lab_defer() {
    [ "$LUCIDOS_WORKSPACE:$LUCIDOS_PRIVATE_DIR" = caller:caller-private ] || return 2
    printf 'deferred %s\n' "$1"
}
lab_step() {
    [ "$LUCIDOS_WORKSPACE:$LUCIDOS_PRIVATE_DIR" = caller:caller-private ] || return 2
    shift
    "$@"
}
`);
  writeFileSync(join(scripts, 'lib/e2e.sh'), `export LUCIDOS_PRIVATE_DIR=disposable-private
E2E_LOCK_BUSY_EXIT=75
setup_e2e_session() {
    [ "$LUCIDOS_PRIVATE_DIR" = disposable-private ] || return 2
    if [ "${'$'}{E2E_BUSY:-}" = 1 ]; then exit "$E2E_LOCK_BUSY_EXIT"; fi
    export LUCIDOS_WORKSPACE=disposable E2E_WORKSPACE=disposable PG_PORT=5511
    trap 'printf "%s:%s" "$LUCIDOS_WORKSPACE" "$LUCIDOS_PRIVATE_DIR" > teardown' EXIT
}
`);
  writeFileSync(join(scripts, 'lib/build_slot_lease.sh'), `build_slot_leased() {
    [ "$LUCIDOS_WORKSPACE:$LUCIDOS_PRIVATE_DIR" = caller:caller-private ] || return 2
    shift
    LUCIDOS_BUILD_SLOT_GRANTED_CLASS=background LUCIDOS_BUILD_SLOT_REQUEST_ID=setup "$@"
}
`);
  mkdirSync(join(scripts, 'scoreboard'));
  writeFileSync(join(scripts, 'scoreboard/lab-cli.mjs'), `
if (process.env.LUCIDOS_WORKSPACE !== 'caller' || process.env.LUCIDOS_PRIVATE_DIR !== 'caller-private') process.exit(2);
console.log(process.argv[2] === 'sample-count' ? '3' : process.argv[2]);
`);
  writeFileSync(join(bin, 'esbuild'), `#!/usr/bin/env bash
for arg; do
    case "$arg" in
        --outfile=*) cat > "\${arg#--outfile=}" <<'DRIVER'
if (process.env.E2E_WORKSPACE !== 'disposable' || process.env.LUCIDOS_PRIVATE_DIR !== 'disposable-private' ||
    process.env.LUCIDOS_EXTERNAL_PG_PORT !== '5511') process.exit(2);
DRIVER
        ;;
    esac
done
`, { mode: 0o755 });
  const run = busy => spawnSync('bash', [join(scripts, 'measure-app.sh'), 'chromium'], {
    env: { ...env, LUCIDOS_WORKSPACE: 'caller', LUCIDOS_PRIVATE_DIR: 'caller-private', E2E_BUSY: busy },
    encoding: 'utf8',
  });
  const deferred = run('1');
  assert.equal(deferred.status, 75, deferred.stdout + deferred.stderr);
  assert.match(deferred.stdout, /deferred setup/);
  assert.throws(() => readFileSync(join(dir, 'teardown'), 'utf8'), { code: 'ENOENT' });

  const result = run('0');
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.match(result.stdout, /verify-release\nrecord\n/);
  assert.equal(readFileSync(join(dir, 'teardown'), 'utf8'), 'disposable:disposable-private');
});

test('the recorded build benchmark restores its probe after success and failure', t => {
  const { dir, env } = fakeLab(t);
  const scripts = join(dir, 'scripts');
  mkdirSync(join(scripts, 'lib'), { recursive: true });
  for (const name of ['measure-engine-build.sh', 'lib/measure_probe.sh', 'lib/lab_slot.sh']) {
    copyFileSync(new URL(`../${name}`, import.meta.url), join(scripts, name));
  }
  for (const name of ['scoreboard', 'with-build-slot.sh']) {
    symlinkSync(join(dir, name), join(scripts, name));
  }
  for (const tail of ['engine/agent_session', 'engine/chat', 'engine/tools', 'llm', 'api/threads']) {
    const folder = join(dir, 'crates/lucidos-fixture/src', tail);
    mkdirSync(folder, { recursive: true });
    writeFileSync(join(folder, 'mod.rs'), 'fn example() {}\n');
  }
  writeFileSync(join(dir, 'cargo'), `#!/usr/bin/env bash
if [ "\${FAIL_PROBE:-0}" = 1 ] &&
    grep -q measure_engine_build_probe crates/lucidos-fixture/src/engine/chat/mod.rs; then
  exit 19
fi
`, { mode: 0o755 });
  writeFileSync(join(dir, '.gitignore'), '/target/\n');

  const git = (...args) => {
    const result = spawnSync('git', ['-c', 'core.hooksPath=/dev/null', ...args], { cwd: dir, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    return result.stdout;
  };
  git('init', '--template=', '-q');
  git('add', '.');
  git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.com', 'commit', '-qm', 'Seed fixture');

  for (const [outcome, failure] of [['success', 0], ['failed', 1]]) {
    const out = join(dir, 'target', outcome);
    const result = spawnSync('bash', ['scripts/measure-engine-build.sh', '--record-lab', '--only', 'edit_chat_build', '--out', out], {
      cwd: dir,
      env: { ...env, FAIL_PROBE: String(failure) },
      encoding: 'utf8',
    });
    assert.equal(result.status, failure ? 19 : 0, result.stdout + result.stderr);
    assert.equal(git('status', '--porcelain'), '');
    if (!failure) {
      const event = JSON.parse(readFileSync(join(out, 'event.json'), 'utf8'));
      assert.equal(event.measurements.edit_chat_build.samples.length, 3);
    }
  }

  const out = join(dir, 'target', 'noop');
  const noop = spawnSync('bash', ['scripts/measure-engine-build.sh', '--record-lab', '--only', 'noop_build', '--out', out], {
    cwd: dir, env, encoding: 'utf8',
  });
  assert.equal(noop.status, 0, noop.stdout + noop.stderr);
  assert.match(noop.stdout, /prepare_warm_build/);
  assert.match(readFileSync(join(out, 'sample-1/results.tsv'), 'utf8'), /^noop_build\t[^\n]+\n$/);
  assert.equal(JSON.parse(readFileSync(join(out, 'event.json'), 'utf8')).step, 'noop_build');
});
