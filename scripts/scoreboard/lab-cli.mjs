import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { APP_RESULT, BUILD_RESULT, SAMPLE_COUNT, buildMeasures, hostFingerprint, idleStatus, labResult, verifyRelease } from './lab.mjs';

function cli(args) {
  return execFileSync('lucidos', args, { encoding: 'utf8', maxBuffer: 8 << 20 });
}

function emit(type, summary, result) {
  process.stdout.write(cli(['events', 'emit', type, '--summary', summary, '--payload', JSON.stringify(result)]));
}

function main([command, ...args]) {
  if (command === 'idle') return idleStatus(cli(['build-slot', '--status'])) ? 0 : 75;
  if (command === 'sample-count') {
    console.log(SAMPLE_COUNT);
    return 0;
  }
  if (command === 'verify-release') {
    const [request, since] = args;
    verifyRelease(JSON.parse(cli(['events', 'query', '--type', 'BuildSlotReleased', '--since', since, '--limit', '1000'])), request);
    return 0;
  }

  const [kind, toolchain, run, context, ...files] = args;
  if (kind !== 'build' && kind !== 'app') throw new Error(`Unknown lab kind: ${kind}`);
  const host = hostFingerprint(toolchain);
  const type = kind === 'build' ? BUILD_RESULT : APP_RESULT;
  if (command === 'defer') {
    emit(type, `${kind} lab deferred before ${context}`, { run, status: 'deferred', host, next_step: context });
  } else if (command === 'record') {
    const contents = files.map(file => readFileSync(file, 'utf8'));
    if (kind === 'build') {
      const measurements = buildMeasures(contents);
      const results = Object.entries(measurements).map(([step, samples]) =>
        labResult({ host, run, packages: context, step, measurements: { [step]: samples }, unit: 'seconds' }));
      for (const result of results) emit(type, `Build lab completed ${result.step}`, result);
    } else {
      const { events: seed_events } = JSON.parse(readFileSync(context, 'utf8'));
      const samples = contents.map(text => JSON.parse(text));
      const { browser, browserVersion: browser_version } = samples[0];
      const measurements = {};
      for (const sample of samples) {
        if (sample.browser !== browser || sample.browserVersion !== browser_version) {
          throw new Error('App lab samples have different browser engines or versions');
        }
        (measurements[sample.journey] ||= []).push(sample.value);
      }
      emit(type, `App lab completed on ${browser}`, labResult({
        host, run, browser, browser_version, seed_events: Number(seed_events), measurements, unit: 'milliseconds',
      }));
    }
  } else {
    throw new Error(`Unknown lab command: ${command}`);
  }
  return 0;
}

try {
  process.exitCode = main(process.argv.slice(2));
} catch (error) {
  console.error(`lab: ${error.message}`);
  process.exitCode = 2;
}
