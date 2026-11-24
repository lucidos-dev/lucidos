/** Temporary diagnostics: docs/temporary-measures.md § Page health log. */
import { CLIENT_BUILD_ID } from 'virtual:build-id';
import { formatBuildId } from './buildId';
import { BASE_PATH } from './basePath';
import { crashStorageKey } from './crashStorage';

type Kind = 'sample' | 'thread-open' | 'media-mount' | 'iframe-mount' | 'answer-card-render' | 'stall' | 'wake' | 'stream' | 'admission' | 'iframe-load' | 'transcript' | 'hide' | 'worker';
const KINDS: Kind[] = ['sample', 'thread-open', 'media-mount', 'iframe-mount', 'answer-card-render', 'stall', 'wake', 'stream', 'admission', 'iframe-load', 'transcript', 'hide', 'worker'];
const FIELDS = new Set(['state', 'phase', 'inflight', 'oldestMs', 'costMs', 'nodes', 'video', 'audio', 'playing', 'loading', 'mediaErrors', 'videoPixels', 'images', 'imagePixels', 'uniqueImagePixels', 'frameVideoLoaded', 'frameVideoPixels', 'frameImagePixels', 'iframes', 'canvas', 'events', 'height', 'blobSources', 'count', 'lagMs', 'ready0', 'ready1', 'ready2', 'ready3', 'ready4', 'network0', 'network1', 'network2', 'network3']);
type Measurements = Record<string, number | string[]>;
interface Entry { at: number; kind: Kind; data: Measurements }
interface RecordData { version: 1; at: number; start?: number; build?: string; samples: Entry[]; events: Entry[] }
type Disk = Pick<Storage, 'getItem' | 'setItem'>;
const bytes = (value: unknown) => new TextEncoder().encode(JSON.stringify(value)).length;

function safeField(k: string, v: unknown): boolean {
  if (k === 'appIds') return Array.isArray(v) && v.length <= 8 && v.every(id => typeof id === 'string' && /^[a-zA-Z0-9_-]{1,48}$/.test(id));
  return FIELDS.has(k) && typeof v === 'number' && Number.isFinite(v) && v >= 0;
}
function numbers(data: Record<string, unknown>): Measurements {
  return Object.fromEntries(Object.entries(data).filter(([k, v]) => safeField(k, v))) as Measurements;
}
function dropEvent(events: Entry[]): void {
  const states: Kind[] = ['wake', 'stream', 'admission', 'hide', 'worker'];
  const index = events.findIndex((entry, i) => !states.includes(entry.kind)
    || events.slice(i + 1).some(later => later.kind === entry.kind));
  events.splice(index < 0 ? 0 : index, 1);
}
function dropOldest(record: RecordData): void {
  if (record.samples.length) record.samples.shift();
  else dropEvent(record.events);
}
const validBuild = (build: unknown): build is string => typeof build === 'string' && /^(?:dev|[a-f0-9]{7,64})$/.test(build);
export class BreadcrumbStore {
  private record: RecordData = { version: 1, at: Date.now(), start: typeof performance !== 'undefined' && performance.timeOrigin > 0 ? performance.timeOrigin : Date.now(), samples: [], events: [] };
  private key: string;
  constructor(private disk: Disk, workspace: string, build = formatBuildId(CLIENT_BUILD_ID)) {
    this.key = crashStorageKey(workspace);
    this.record.build = validBuild(build) ? build : 'dev';
  }
  read(): RecordData | null {
    try {
      const raw = this.disk.getItem(this.key);
      if (!raw || raw.length > 2300) return null;
      const value = JSON.parse(raw);
      if (value.version !== 1 || !Number.isFinite(value.at) || Date.now() - value.at > 86_400_000 || value.at > Date.now() + 60_000) return null;
      if (value.start !== undefined && (!Number.isFinite(value.start) || value.start < 0)) return null;
      if (value.build !== undefined && !validBuild(value.build)) return null;
      if (!Array.isArray(value.samples) || !Array.isArray(value.events) || value.samples.length > 6 || value.events.length > 12) return null;
      const valid = (e: Entry) => e && Number.isFinite(e.at) && KINDS.includes(e.kind) && e.data && typeof e.data === 'object' && !Array.isArray(e.data) && Object.keys(e.data).every(k => safeField(k, e.data[k]));
      if (![...value.samples, ...value.events].every(valid) || bytes(value) > 2300) return null;
      const copy = (e: Entry): Entry => ({ at: e.at, kind: e.kind, data: numbers(e.data) });
      return { version: 1, at: value.at, start: value.start ?? 0, ...(value.build === undefined ? {} : { build: value.build }), samples: value.samples.map(copy), events: value.events.map(copy) };
    } catch { return null; }
  }
  add(kind: Kind, data: Record<string, unknown>): void {
    const list = kind === 'sample' ? this.record.samples : this.record.events;
    list.push({ at: Date.now(), kind, data: numbers(kind === 'sample' ? Object.fromEntries(Object.entries(data).filter(([, v]) => v !== 0)) : data) });
    const limit = kind === 'sample' ? 6 : 12;
    if (list.length > limit) {
      if (kind === 'sample') list.shift();
      else dropEvent(list);
    }
    this.record.at = Date.now();
    while (bytes(this.record) > 2300) dropOldest(this.record);
  }
  flush(): void {
    try { this.disk.setItem(this.key, JSON.stringify(this.record)); } catch {  }
  }
}
export function crashStore(): BreadcrumbStore {
  return new BreadcrumbStore({ getItem: k => localStorage.getItem(k), setItem: (k, v) => localStorage.setItem(k, v) }, BASE_PATH);
}

export function recoveredStartup(data: Record<string, unknown>, store = crashStore()): Record<string, unknown> {
  const prior = store.read();
  store.flush();
  if (data.kind !== 'likely_crash' || !prior) return data;
  const result = { ...data, breadcrumb: prior };
  while (bytes(result) > 3900 && (prior.samples.length || prior.events.length)) {
    dropOldest(prior);
  }
  return bytes(result) <= 3900 ? result : data;
}
