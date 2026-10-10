/** A test `XMLHttpRequest` that routes through `globalThis.fetch`. Suites that
 *  stub `fetch` to drive the whole client keep working for the one call that
 *  uses XHR, the image upload (`postWithUploadProgress`). Not a test file. */

type Handler = ((e: unknown) => void) | null;

class XhrOverFetch {
  upload: { onprogress: Handler; onload: Handler } = { onprogress: null, onload: null };
  onload: Handler = null;
  onerror: Handler = null;
  onabort: Handler = null;
  status = 0;
  statusText = '';
  responseText = '';
  private method = 'GET';
  private url = '';
  private headers: Record<string, string> = {};
  private responseHeaders = '';
  private aborted = false;

  open(method: string, url: string) { this.method = method; this.url = url; }
  setRequestHeader(name: string, value: string) { this.headers[name] = value; }
  getAllResponseHeaders() { return this.responseHeaders; }
  abort() {
    this.aborted = true;
    this.onabort?.({});
  }

  send(body: unknown) {
    fetch(this.url, { method: this.method, headers: this.headers, body: body as BodyInit })
      .then(async (res) => {
        if (this.aborted) return;
        this.upload.onprogress?.({ lengthComputable: true, loaded: 1, total: 1 });
        this.upload.onload?.({});
        this.status = res.status;
        this.statusText = res.statusText;
        this.responseText = await res.text();
        const lines: string[] = [];
        res.headers.forEach((value, name) => lines.push(`${name}: ${value}`));
        this.responseHeaders = lines.join('\r\n');
        this.onload?.({});
      })
      .catch(() => {
        if (!this.aborted) this.onerror?.({});
      });
  }
}

let saved: unknown;

export function installXhrOverFetch(): void {
  saved = (globalThis as { XMLHttpRequest?: unknown }).XMLHttpRequest;
  (globalThis as { XMLHttpRequest?: unknown }).XMLHttpRequest = XhrOverFetch;
}

export function uninstallXhrOverFetch(): void {
  (globalThis as { XMLHttpRequest?: unknown }).XMLHttpRequest = saved;
}
