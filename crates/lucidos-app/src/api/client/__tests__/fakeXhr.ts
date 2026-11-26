import { vi } from 'vitest';

type Handler = ((e: any) => void) | null;

export class FakeXhr {
  static last: FakeXhr | null = null;
  upload: { onprogress: Handler; onload: Handler } = { onprogress: null, onload: null };
  onload: Handler = null;
  onerror: Handler = null;
  onabort: Handler = null;
  method = '';
  url = '';
  requestHeaders: Record<string, string> = {};
  body: unknown = null;
  aborted = false;
  status = 0;
  statusText = '';
  responseText = '';
  private rawHeaders = '';

  open(method: string, url: string) { this.method = method; this.url = url; }
  setRequestHeader(k: string, v: string) { this.requestHeaders[k] = v; }
  send(body: unknown) { this.body = body; FakeXhr.last = this; }
  abort() { this.aborted = true; this.onabort?.({}); }
  getAllResponseHeaders() { return this.rawHeaders; }

  progress(loaded: number, total: number) {
    this.upload.onprogress?.({ lengthComputable: true, loaded, total });
  }
  bodySent() { this.upload.onload?.({}); }
  respond(status: number, body: string, headers: Record<string, string> = {}) {
    this.status = status;
    this.responseText = body;
    this.rawHeaders = Object.entries(headers).map(([k, v]) => `${k}: ${v}`).join('\r\n');
    this.onload?.({});
  }
  dropConnection() { this.onerror?.({}); }
}

export function current(): FakeXhr {
  if (!FakeXhr.last) throw new Error('no request was sent');
  return FakeXhr.last;
}

export async function sent(): Promise<FakeXhr> {
  await vi.advanceTimersByTimeAsync(0);
  return current();
}

export function installFakeXhr(): void {
  FakeXhr.last = null;
  (globalThis as any).XMLHttpRequest = FakeXhr;
}

export function uninstallFakeXhr(): void {
  delete (globalThis as any).XMLHttpRequest;
}
