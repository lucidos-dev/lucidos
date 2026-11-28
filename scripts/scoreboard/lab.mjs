import { createHash } from 'node:crypto';
import { arch, cpus, hostname, platform, release, totalmem } from 'node:os';
import { PERF_VALUE_FIELD } from './perf-samples.mjs';

export const BUILD_RESULT = 'BuildLabRunCompleted';
export const APP_RESULT = 'AppLabRunCompleted';
export const SAMPLE_COUNT = 3;

export function idleStatus(text) {
  const header = /^build slots: (\d+)\/[1-9]\d* held, capacity from .+$/m.exec(text);
  if (!header) throw new Error('Unrecognized build-slot status');
  return Number(header[1]) === 0 && !/^(?:at least one build|a priority build) is waiting\b/m.test(text);
}

export function verifyRelease(events, request) {
  const release = events.find(event => event.event_type === 'BuildSlotReleased' && event.payload.request_id === request);
  if (!release) throw new Error('No slot release event for the lab step');
  if (release.payload.class !== 'background' || release.payload.exit_code !== 0) {
    throw new Error('The lab step needs a successful background slot release');
  }
}

export function hostFingerprint(toolchain) {
  const host = { os: `${platform()} ${release()}`, architecture: arch(), cores: cpus().length, memory_bytes: totalmem(), toolchain };
  const fingerprint = createHash('sha256').update(JSON.stringify([hostname(), host])).digest('hex');
  return { ...host, fingerprint };
}

export function labResult({ host, measurements, unit, ...details }) {
  return {
    ...details,
    status: 'completed',
    host,
    measurements: Object.fromEntries(Object.entries(measurements).map(([name, samples]) => {
      if (samples.length < SAMPLE_COUNT) throw new Error(`${name} needs at least ${SAMPLE_COUNT} samples`);
      if (!samples.every(sample => Number.isFinite(sample) && sample >= 0)) {
        throw new Error(`${name} needs finite, nonnegative samples`);
      }
      const sorted = [...samples].sort((a, b) => a - b);
      const median = (sorted[Math.floor((sorted.length - 1) / 2)] + sorted[Math.floor(sorted.length / 2)]) / 2;
      return [name, { unit, samples, median }];
    })),
  };
}

export function buildMeasures(passes) {
  const runs = passes.map(pass => pass.trim().split('\n').map(row => row.split('\t')));
  const first = runs[0];
  if (first.some(([step, seconds]) => !step || !seconds) ||
      new Set(first.map(([step]) => step)).size !== first.length ||
      runs.some(run => run.length !== first.length ||
        run.some(([name, , jobs], i) => name !== first[i][0] || jobs !== first[i][2]))) {
    throw new Error('Build passes have different steps or job counts');
  }
  return Object.fromEntries(first.map(([name], index) => [
    name, runs.map(run => run[index][1] ? Number(run[index][1]) : NaN),
  ]));
}

export function appMark(journey, { category, message, data }, thread) {
  if (category !== 'perf' || !data) return undefined;
  switch (journey) {
    case 'cold-boot':
      return message === 'boot' && data.deepLink === false ? data[PERF_VALUE_FIELD.boot] : undefined;
    case 'notification-deep-link':
      return message === 'boot' && data.deepLink === true ? data.landedMs : undefined;
    case 'thread-navigation':
      return message === 'thread-render' && data.threadId === thread ? data[PERF_VALUE_FIELD['thread-render']] : undefined;
  }
}
