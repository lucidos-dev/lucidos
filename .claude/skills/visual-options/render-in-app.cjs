// Renders a mockup inside the user's running Lucidos, read-only, and saves a
// screenshot. It injects a stylesheet and an HTML fragment into the page and
// saves nothing to the workspace. Usage lives in SKILL.md beside this file.
//
//   node render-in-app.cjs --url <workspace url> --out <png>
//     [--hash thread=<uuid>] [--css <file>] [--html <file>]
//     [--into <selector>] [--capture <selector>] [--hover <selector>]
//     [--width 1300] [--height 1000]

const path = require('node:path');
const { readFileSync } = require('node:fs');

const repoRoot = path.resolve(__dirname, '../../..');
const { chromium } = require(path.join(repoRoot, 'node_modules/playwright'));

function args() {
  const out = {};
  const argv = process.argv.slice(2);
  for (let i = 0; i < argv.length; i += 2) {
    const [key, value] = [argv[i], argv[i + 1]];
    if (!key.startsWith('--') || value === undefined || value.startsWith('--')) {
      throw new Error(`every flag takes a value: ${key}`);
    }
    out[key.slice(2)] = value;
  }
  if (!out.url || !out.out) throw new Error('--url and --out are required');
  return out;
}

(async () => {
  const a = args();
  const css = a.css ? readFileSync(a.css, 'utf8') : '';
  const html = a.html ? readFileSync(a.html, 'utf8') : '';
  const into = a.into || '.thread-feed';
  const capture = a.capture || '.thread-content';

  const browser = await chromium.launch();
  const page = await browser.newPage({
    viewport: { width: Number(a.width || 1300), height: Number(a.height || 1000) },
    deviceScaleFactor: 2,
  });
  // `load`, never `networkidle`: the app holds an SSE stream open.
  await page.goto(`${a.url.replace(/\/+$/, '')}/${a.hash ? `#${a.hash}` : ''}`, { waitUntil: 'load' });
  await page.waitForSelector(into, { timeout: 20000 });
  await page.waitForTimeout(2500);
  await page.evaluate(({ css, html, into, capture }) => {
    const style = document.createElement('style');
    style.textContent = css;
    document.head.appendChild(style);
    document.querySelectorAll('.toast-container').forEach((t) => t.remove());
    if (html) document.querySelector(into).insertAdjacentHTML('beforeend', html);
    const scroller = document.querySelector(capture);
    scroller.scrollTop = scroller.scrollHeight;
  }, { css, html, into, capture });
  await page.waitForTimeout(400);
  if (a.hover) await page.hover(a.hover);
  await page.locator(capture).first().screenshot({ path: a.out });
  await browser.close();
})().catch((e) => {
  console.error(e);
  process.exit(1);
});
