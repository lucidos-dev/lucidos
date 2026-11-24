import { execFile } from 'node:child_process';
import { appendFileSync, mkdirSync } from 'node:fs';
import { dirname, relative } from 'node:path';
import type { FullConfig, Reporter, Suite, TestCase, TestResult } from '@playwright/test/reporter';

export interface MemSample {
  pages: number;
  limit: number;
  gpuCount: number | null;
  gpuRssKb: number | null;
}

export type ReadSample = () => Promise<MemSample | null>;

export type Env = Record<string, string | undefined>;

export interface MemSampleReporterOptions {
  platform?: string;
  env?: Env;
  readSample?: ReadSample;
  now?: () => Date;
}

export interface MemLine {
  time: Date;
  project: string;
  spec: string;
  title: string;
  retry: number;
  status: string;
  durationMs: number;
  sample: MemSample;
  delta: number | null;
}

const CHILD_TIMEOUT_MS = 2_000;
const PAGES_SYSCTL = 'vm.compressor.pages_compressed';
const LIMIT_SYSCTL = 'vm.compressor.pages_compressed_limit';

const clean = (s: string) => s.replace(/[\t\r\n]+/g, ' ');
const orUnknown = (n: number | null) => (n === null ? '?' : String(n));

export function formatMemLine(l: MemLine): string {
  return [
    l.time.toISOString(),
    `project=${clean(l.project)}`,
    `spec=${clean(l.spec)}`,
    `title=${clean(l.title)}`,
    `retry=${l.retry}`,
    `status=${l.status}`,
    `duration_ms=${l.durationMs}`,
    `pages=${l.sample.pages}`,
    `limit=${l.sample.limit}`,
    `delta=${orUnknown(l.delta)}`,
    `webkit_gpu=${orUnknown(l.sample.gpuCount)}`,
    `webkit_gpu_rss_kb=${orUnknown(l.sample.gpuRssKb)}`,
  ].join('\t');
}

export function parseSysctl(stdout: string): { pages: number; limit: number } | null {
  const [pages, limit] = stdout.trim().split('\n').map((v) => Number(v.trim()));
  return Number.isFinite(pages) && Number.isFinite(limit) ? { pages, limit } : null;
}

export function parseGpuProcesses(stdout: string, token: string): { count: number; rssKb: number } {
  let count = 0;
  let rssKb = 0;
  for (const row of stdout.split('\n')) {
    const m = /^\s*(\d+)\s+(.+)$/.exec(row);
    if (!m) continue;
    const exe = m[2];
    const name = exe.slice(exe.lastIndexOf('/') + 1);
    if (exe.includes(token) && name.includes('WebKit.GPU')) {
      count += 1;
      rssKb += Number(m[1]);
    }
  }
  return { count, rssKb };
}

function run(cmd: string, args: string[]): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(cmd, args, { timeout: CHILD_TIMEOUT_MS, maxBuffer: 8 * 1024 * 1024 }, (err, stdout) =>
      err ? reject(err) : resolve(stdout),
    );
  });
}

export function systemReadSample(env: Env): ReadSample {
  const token = env.LUCIDOS_E2E_WEBKIT_PATH_TOKEN;
  return async () => {
    const [sysctl, ps] = await Promise.allSettled([
      run('sysctl', ['-n', PAGES_SYSCTL, LIMIT_SYSCTL]),
      token ? run('ps', ['-Ao', 'rss=,comm=']) : Promise.reject(new Error('no WebKit path token')),
    ]);
    const vm = sysctl.status === 'fulfilled' ? parseSysctl(sysctl.value) : null;
    if (!vm) return null;
    const gpu = ps.status === 'fulfilled' && token ? parseGpuProcesses(ps.value, token) : null;
    return { ...vm, gpuCount: gpu?.count ?? null, gpuRssKb: gpu?.rssKb ?? null };
  };
}

function testTitle(test: TestCase): string {
  const parts = [test.title];
  for (let s: Suite | undefined = test.parent; s && s.type === 'describe'; s = s.parent) {
    parts.unshift(s.title);
  }
  return parts.join(' > ');
}

export default class MemSampleReporter implements Reporter {
  private readonly enabled: boolean;
  private readonly logPaths: string[];
  private readonly readSample: ReadSample;
  private readonly now: () => Date;
  private rootDir = '';
  private baseline: number | null = null;
  private readonly lastByWorker = new Map<number, number>();
  private queue: Promise<void> = Promise.resolve();

  constructor(options: MemSampleReporterOptions = {}) {
    const env = options.env ?? process.env;
    this.logPaths = [env.LUCIDOS_E2E_MEM_LOG, env.LUCIDOS_E2E_MEM_MIRROR].filter((p): p is string => !!p);
    this.enabled = (options.platform ?? process.platform) === 'darwin' && this.logPaths.length > 0;
    this.readSample = options.readSample ?? systemReadSample(env);
    this.now = options.now ?? (() => new Date());
  }

  printsToStdio(): boolean {
    return false;
  }

  onBegin(config: FullConfig): void {
    if (!this.enabled) return;
    this.rootDir = config.rootDir;
    this.enqueue(async () => {
      this.baseline = (await this.readSample())?.pages ?? null;
    });
  }

  onTestEnd(test: TestCase, result: TestResult): void {
    if (!this.enabled) return;
    try {
      const worker = result.parallelIndex;
      const fields = {
        time: this.now(),
        project: test.parent.project()?.name ?? '',
        spec: relative(this.rootDir, test.location.file),
        title: testTitle(test),
        retry: result.retry,
        status: result.status,
        durationMs: result.duration,
      };
      this.enqueue(async () => {
        const sample = await this.readSample();
        if (!sample) return;
        const previous = this.lastByWorker.get(worker) ?? this.baseline;
        this.lastByWorker.set(worker, sample.pages);
        const delta = previous === null ? null : sample.pages - previous;
        this.append(formatMemLine({ ...fields, sample, delta }));
      });
    } catch {
    }
  }

  async onEnd(): Promise<void> {
    await this.queue;
  }

  private enqueue(task: () => Promise<void>): void {
    this.queue = this.queue.then(task).catch(() => {});
  }

  private append(line: string): void {
    for (const path of this.logPaths) {
      try {
        mkdirSync(dirname(path), { recursive: true });
        appendFileSync(path, `${line}\n`);
      } catch {
      }
    }
  }
}
