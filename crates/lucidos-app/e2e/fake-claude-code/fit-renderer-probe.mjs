// Disposable acceptance launcher. No tracing/HAR, profile persistence, secret
// logging or credentials in the shell. The real shell and SDK do all layout.
import { request as httpRequest } from 'node:http';
import { request as httpsRequest } from 'node:https';
import { pathToFileURL } from 'node:url';

export function fitReportHeaders(request, mainFrame, base, device, credential) {
  const url = new URL(request.url());
  const expected = new URL('api/v1/widgets/fit-report', base);
  if (request.frame() !== mainFrame || request.method() !== 'POST'
    || url.origin !== expected.origin || url.pathname !== expected.pathname
    || url.search || request.postDataJSON()?.device_id !== device) return null;
  return { ...request.headers(), 'x-lucidos-fit-renderer': credential };
}

function call(base, path, env, body, extra = {}) {
  return new Promise((resolve, reject) => {
    const url = new URL(path.replace(/^\//, ''), base);
    const send = url.protocol === 'https:' ? httpsRequest : httpRequest;
    const req = send(url, { method: 'POST', rejectUnauthorized: false, headers: {
      'content-type': 'application/json', 'x-lucidos-agent-origin-token': env.LUCIDOS_AGENT_ORIGIN_TOKEN, ...extra,
    } }, res => {
      let text = '';
      res.on('data', chunk => { text += chunk; if (text.length > 131072) req.destroy(); });
      res.on('end', () => {
        try { resolve({ status: res.statusCode, body: JSON.parse(text) }); }
        catch { reject(new Error('invalid fixture response')); }
      });
      res.on('error', () => reject(new Error('fixture response failed')));
    });
    req.on('error', () => reject(new Error('fixture request failed')));
    req.setTimeout(45000, () => req.destroy());
    req.end(JSON.stringify(body));
  });
}

export function fitRendererBase(env) {
  let address = env.LUCIDOS_API_BASE_URL;
  if (!address) {
    const port = env.LUCIDOS_API_PORT;
    if (!/^\d+$/.test(port ?? '') || Number(port) < 1 || Number(port) > 65535) throw new Error('engine port required');
    const tls = env.LUCIDOS_TLS_CERT?.trim() && env.LUCIDOS_TLS_KEY?.trim();
    address = `${tls ? 'https' : 'http'}://127.0.0.1:${port}`;
  }
  const base = new URL(address);
  if (!['http:', 'https:'].includes(base.protocol) || !['localhost', '127.0.0.1', '[::1]'].includes(base.hostname)
    || base.username || base.password || base.search || base.hash) throw new Error('local engine URL required');
  if (!base.pathname.endsWith('/')) base.pathname += '/';
  return base;
}

export async function probeFitRenderer(widgets, env = process.env, browserName = 'chromium') {
  let browser;
  let stage = 'origin';
  let current = null;
  try {
    if (!env.LUCIDOS_AGENT_ORIGIN_TOKEN || !env.LUCIDOS_THREAD_ID) return { verified: false, stage };
    stage = 'address';
    const base = fitRendererBase(env);
    stage = 'browser';
    if (!['chromium', 'webkit'].includes(browserName)) return { verified: false, stage };
    const { chromium, webkit } = await import('@playwright/test');
    browser = await ({ chromium, webkit })[browserName].launch({ headless: true });
    const context = await browser.newContext({ ignoreHTTPSErrors: true, serviceWorkers: 'block' });
    const page = await context.newPage();
    stage = 'shell';
    await page.goto(base.href);
    stage = 'device';
    await page.waitForFunction(() => localStorage.getItem('lucidos-device-id'));
    const device = await page.evaluate(() => localStorage.getItem('lucidos-device-id'));
    stage = 'event-stream';
    await page.waitForFunction(() => document.documentElement.dataset.lucidosEventStream === 'connected');
    const results = [];
    for (const widget of widgets) {
      current = widget.id;
      stage = 'registration';
      const registered = await call(base, '/api/v1/widgets/fit-session', env, {
        thread_id: env.LUCIDOS_THREAD_ID, app_id: widget.id, device_id: device,
      });
      if (registered.status !== 200) return { verified: false, stage, status: registered.status };
      const { renderer_session_id: handle, credential } = registered.body;
      await page.evaluate(handle => sessionStorage.setItem('lucidos-fit-renderer-session', handle), widget.noReport ? '' : handle);
      const reports = [];
      const refused = [];
      await page.route('**/api/v1/widgets/fit-report', async route => {
        const request = route.request();
        const headers = fitReportHeaders(request, page.mainFrame(), base, device, credential);
        if (!headers) { await route.continue(); return; }
        const body = request.postDataJSON();
        reports.push(body);
        // An attacker reaches the real route BEFORE the genuine network POST.
        const wrong = await call(base, '/api/v1/widgets/fit-report', env, body, { 'x-lucidos-fit-renderer': 'invalid-credential' });
        refused.push(wrong.status);
        const unattested = await call(base, '/api/v1/widgets/fit-report', env, body);
        refused.push(unattested.status);
        await route.continue({ headers });
      });
      stage = 'render';
      const response = widget.noReport ? null : page.waitForResponse(res => res.url() === new URL('api/v1/widgets/fit-report', base).href, { timeout: 45000 });
      const checked = await call(base, '/api/v1/widgets/fit', env, {
        thread_id: env.LUCIDOS_THREAD_ID, app_id: widget.id, renderer_session_id: handle, device_id: device,
      });
      const status = response ? (await response).status() : null;
      const replay = reports.length ? await call(base, '/api/v1/widgets/fit-report', env, reports[0], { 'x-lucidos-fit-renderer': credential }) : null;
      results.push({ app: widget.id, status, operationStatus: checked.status, note: checked.body.note, refused, replayStatus: replay?.status, reports });
      await page.unroute('**/api/v1/widgets/fit-report');
      await page.evaluate(() => sessionStorage.removeItem('lucidos-fit-renderer-session'));
    }
    return { verified: true, responder: 'agent-renderer', device, results };
  } catch (error) { return { verified: false, stage, app: current, error: String(error?.message ?? error).split('\n')[0].slice(0, 200) }; }
  finally { if (browser) await browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const result = await probeFitRenderer(JSON.parse(process.argv[2]), process.env, process.argv[3]);
  process.stdout.write(JSON.stringify(result));
}
