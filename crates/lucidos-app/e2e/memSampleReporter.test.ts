import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { FullConfig, Suite, TestCase, TestResult } from '@playwright/test/reporter';
import MemSampleReporter, {
  formatMemLine,
  parseGpuProcesses,
  parseSysctl,
  type Env,
  type MemSample,
} from './memSampleReporter';

const ROOT = '/repo/crates/lucidos-app/e2e';
const config = { rootDir: ROOT } as FullConfig;

function fakeTest(title: string, opts: { file?: string; project?: string; describe?: string } = {}): TestCase {
  const project = { title: opts.project ?? 'mobile-webkit', type: 'project', project: () => ({ name: opts.project ?? 'mobile-webkit' }) };
  const file = { title: 'chat.spec.ts', type: 'file', parent: project, project: project.project };
  const parent = opts.describe
    ? { title: opts.describe, type: 'describe', parent: file, project: project.project }
    : file;
  return {
    title,
    parent: parent as unknown as Suite,
    location: { file: join(ROOT, opts.file ?? 'chat.spec.ts'), line: 1, column: 1 },
  } as TestCase;
}

function fakeResult(parallelIndex: number, extra: Partial<TestResult> = {}): TestResult {
  return { parallelIndex, workerIndex: parallelIndex, status: 'passed', duration: 1234, retry: 0, ...extra } as TestResult;
}

function sample(pages: number): MemSample {
  return { pages, limit: 19_748_080, gpuCount: 1, gpuRssKb: 2048 };
}

/** A sampler that hands out the given page counts in order. */
function samplerOf(...pages: number[]) {
  return vi.fn(async () => sample(pages.shift() ?? 0));
}

let dir: string;
let env: Env;
const log = () => join(dir, 'run', 'mem-samples.log');
const mirror = () => join(dir, 'home', '.lucidos', 'e2e-mem', 'run-1.log');
const lines = (path = log()) => readFileSync(path, 'utf8').trimEnd().split('\n');
const field = (line: string, key: string) =>
  line.split('\t').find((f) => f.startsWith(`${key}=`))?.slice(key.length + 1);

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), 'mem-sample-'));
  env = { LUCIDOS_E2E_MEM_LOG: log(), LUCIDOS_E2E_MEM_MIRROR: mirror() };
});
afterEach(() => rmSync(dir, { recursive: true, force: true }));

describe('formatMemLine', () => {
  it('writes every field as one tab-separated line, folding whitespace in the title', () => {
    const line = formatMemLine({
      time: new Date('2026-10-07T04:30:00.000Z'),
      project: 'mobile-webkit',
      spec: 'chat.spec.ts',
      title: 'sends\ta\nmessage',
      retry: 1,
      status: 'failed',
      durationMs: 4200,
      sample: { pages: 650_000, limit: 19_748_080, gpuCount: 2, gpuRssKb: 512_000 },
      delta: 1500,
    });
    expect(line.split('\t')).toEqual([
      '2026-10-07T04:30:00.000Z',
      'project=mobile-webkit',
      'spec=chat.spec.ts',
      'title=sends a message',
      'retry=1',
      'status=failed',
      'duration_ms=4200',
      'pages=650000',
      'limit=19748080',
      'delta=1500',
      'webkit_gpu=2',
      'webkit_gpu_rss_kb=512000',
    ]);
  });

  it('marks unknown values with ?', () => {
    const line = formatMemLine({
      time: new Date(0), project: 'p', spec: 's', title: 't', retry: 0, status: 'passed', durationMs: 0,
      sample: { pages: 1, limit: 2, gpuCount: null, gpuRssKb: null },
      delta: null,
    });
    expect(field(line, 'delta')).toBe('?');
    expect(field(line, 'webkit_gpu')).toBe('?');
    expect(field(line, 'webkit_gpu_rss_kb')).toBe('?');
  });
});

describe('parseSysctl', () => {
  it('reads both values', () => {
    expect(parseSysctl('494044\n19748080\n')).toEqual({ pages: 494_044, limit: 19_748_080 });
  });
  it('rejects output that is not two numbers', () => {
    expect(parseSysctl('')).toBeNull();
    expect(parseSysctl('sysctl: unknown oid\n')).toBeNull();
  });
});

describe('parseGpuProcesses', () => {
  const token = 'ms-playwright/webkit';
  const gpu = '/Users/me/Library/Caches/ms-playwright/webkit-2191/Playwright.app/Contents/Frameworks/WebKit.framework/Versions/A/XPCServices/com.apple.WebKit.GPU.xpc/Contents/MacOS/com.apple.WebKit.GPU.Development';

  it('counts only Playwright WebKit GPU executables and sums their RSS', () => {
    const ps = [
      `  1000 ${gpu}`,
      `   500 ${gpu}`,
      ` 9999 /Users/me/Library/Caches/ms-playwright/webkit-2191/Playwright.app/Contents/MacOS/Playwright`,
      ` 8888 /System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices/com.apple.WebKit.GPU.xpc/Contents/MacOS/com.apple.WebKit.GPU`,
      ` 7777 /Users/me/Library/Caches/ms-playwright/chromium-1200/chrome-mac/Chromium.app/Contents/MacOS/Chromium`,
      'garbage',
    ].join('\n');
    expect(parseGpuProcesses(ps, token)).toEqual({ count: 2, rssKb: 1500 });
  });

  it('matches on the token it is given, so a custom browsers path finds nothing in the default cache', () => {
    expect(parseGpuProcesses(`10 ${gpu}`, '/opt/pw/webkit')).toEqual({ count: 0, rssKb: 0 });
  });
});

describe('MemSampleReporter', () => {
  it('appends one line per test to the log and the mirror', async () => {
    const r = new MemSampleReporter({ platform: 'darwin', env, readSample: samplerOf(100, 150), now: () => new Date(0) });
    r.onBegin(config);
    r.onTestEnd(fakeTest('sends a message', { describe: 'chat' }), fakeResult(0));
    await r.onEnd();

    const [line] = lines();
    expect(lines(mirror())).toEqual([line]);
    expect(field(line, 'project')).toBe('mobile-webkit');
    expect(field(line, 'spec')).toBe('chat.spec.ts');
    expect(field(line, 'title')).toBe('chat > sends a message');
    expect(field(line, 'status')).toBe('passed');
    expect(field(line, 'duration_ms')).toBe('1234');
    expect(field(line, 'pages')).toBe('150');
    expect(field(line, 'delta')).toBe('50');
  });

  it('takes exactly one sample per test end, plus one at the start', async () => {
    const read = samplerOf(0, 1, 2, 3);
    const r = new MemSampleReporter({ platform: 'darwin', env, readSample: read });
    r.onBegin(config);
    for (const t of ['a', 'b', 'c']) r.onTestEnd(fakeTest(t), fakeResult(0));
    await r.onEnd();
    expect(read).toHaveBeenCalledTimes(4);
  });

  it("measures each worker against its own previous sample, and a first test against the start", async () => {
    // start=100; w0 → 110; w1 → 130; w0 → 135; w1 → 200
    const r = new MemSampleReporter({ platform: 'darwin', env, readSample: samplerOf(100, 110, 130, 135, 200) });
    r.onBegin(config);
    r.onTestEnd(fakeTest('w0-a'), fakeResult(0));
    r.onTestEnd(fakeTest('w1-a'), fakeResult(1));
    r.onTestEnd(fakeTest('w0-b'), fakeResult(0));
    r.onTestEnd(fakeTest('w1-b'), fakeResult(1));
    await r.onEnd();
    expect(lines().map((l) => [field(l, 'title'), field(l, 'delta')])).toEqual([
      ['w0-a', '10'],
      ['w1-a', '30'],
      ['w0-b', '25'],
      ['w1-b', '70'],
    ]);
  });

  it('measures a retry in a replaced worker from the test before it', async () => {
    const r = new MemSampleReporter({ platform: 'darwin', env, readSample: samplerOf(100, 400, 410) });
    r.onBegin(config);
    r.onTestEnd(fakeTest('fails'), fakeResult(0, { status: 'failed' }));
    r.onTestEnd(fakeTest('fails'), fakeResult(0, { workerIndex: 1, retry: 1 }));
    await r.onEnd();
    expect(lines().map((l) => [field(l, 'retry'), field(l, 'delta')])).toEqual([
      ['0', '300'],
      ['1', '10'],
    ]);
  });

  it('writes an unknown delta when the start sample failed', async () => {
    const read = vi.fn<() => Promise<MemSample | null>>()
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce(sample(500));
    const r = new MemSampleReporter({ platform: 'darwin', env, readSample: read });
    r.onBegin(config);
    r.onTestEnd(fakeTest('a'), fakeResult(0));
    await r.onEnd();
    expect(field(lines()[0], 'delta')).toBe('?');
  });

  it('survives a sampler that throws or rejects, writing nothing for that test', async () => {
    const read = vi.fn<() => Promise<MemSample | null>>()
      .mockResolvedValueOnce(sample(100))
      .mockRejectedValueOnce(new Error('sysctl gone'))
      .mockImplementationOnce(() => { throw new Error('spawn failed'); })
      .mockResolvedValueOnce(sample(120));
    const r = new MemSampleReporter({ platform: 'darwin', env, readSample: read });
    r.onBegin(config);
    expect(() => {
      r.onTestEnd(fakeTest('rejects'), fakeResult(0));
      r.onTestEnd(fakeTest('throws'), fakeResult(0));
      r.onTestEnd(fakeTest('ok'), fakeResult(0));
    }).not.toThrow();
    await expect(r.onEnd()).resolves.toBeUndefined();
    expect(lines().map((l) => [field(l, 'title'), field(l, 'delta')])).toEqual([['ok', '20']]);
  });

  it('is a silent no-op off macOS: no sample, no file', async () => {
    const read = samplerOf(1, 2);
    const r = new MemSampleReporter({ platform: 'linux', env, readSample: read });
    r.onBegin(config);
    r.onTestEnd(fakeTest('a'), fakeResult(0));
    await r.onEnd();
    expect(read).not.toHaveBeenCalled();
    expect(existsSync(log())).toBe(false);
    expect(existsSync(mirror())).toBe(false);
  });

  it('is a no-op when the harness named no log', async () => {
    const read = samplerOf(1, 2);
    const r = new MemSampleReporter({ platform: 'darwin', env: {}, readSample: read });
    r.onBegin(config);
    r.onTestEnd(fakeTest('a'), fakeResult(0));
    await r.onEnd();
    expect(read).not.toHaveBeenCalled();
  });
});
