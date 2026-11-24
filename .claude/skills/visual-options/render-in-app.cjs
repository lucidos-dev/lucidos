
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
  if (out.packaged && out.packaged !== 'mac') throw new Error('--packaged takes "mac", the one packaged build');
  return out;
}

function macTitlebar() {
  const read = (rel, pattern, what) => {
    const match = pattern.exec(readFileSync(path.join(repoRoot, rel), 'utf8'));
    if (!match) throw new Error(`cannot find ${what} in ${rel}`);
    return Number(match[1]);
  };
  return {
    insetPx: read('crates/lucidos-app/src/lib.rs', /'--titlebar-inset','(\d+)px'/, 'the title-bar inset'),
    lightsXPx: read('crates/lucidos-app/src/traffic_lights.rs', /const LIGHTS_X_PX: f64 = ([\d.]+);/, 'LIGHTS_X_PX'),
  };
}

(async () => {
  const a = args();
  const css = a.css ? readFileSync(a.css, 'utf8') : '';
  const html = a.html ? readFileSync(a.html, 'utf8') : '';
  const into = a.into || '.thread-feed';
  const capture = a.capture || '.thread-content';
  const mac = a.packaged === 'mac' ? macTitlebar() : null;

  const browser = await chromium.launch();
  const page = await browser.newPage({
    viewport: { width: Number(a.width || 1300), height: Number(a.height || 1000) },
    deviceScaleFactor: 2,
  });
  await page.goto(`${a.url.replace(/\/+$/, '')}/${a.hash ? `#${a.hash}` : ''}`, { waitUntil: 'load' });
  await page.waitForSelector(into, { timeout: 20000 });
  await page.waitForTimeout(2500);
  await page.evaluate(({ scale, mac }) => {
    const root = document.documentElement;
    if (scale) root.style.setProperty('--user-ui-scale', `${scale}%`);
    if (!mac) return;
    root.style.setProperty('--titlebar-inset', `${mac.insetPx}px`);
    root.style.setProperty('--titlebar-lights-x', `${mac.lightsXPx}px`);
    root.setAttribute('data-titlebar-overlay', '');
  }, { scale: a['ui-scale'], mac });
  await page.waitForTimeout(600);
  await page.evaluate(({ css, html, into, capture, mac }) => {
    const style = document.createElement('style');
    style.textContent = css;
    document.head.appendChild(style);
    document.querySelectorAll('.toast-container').forEach((t) => t.remove());
    if (html) document.querySelector(into).insertAdjacentHTML('beforeend', html);
    const scroller = capture === 'viewport' ? null : document.querySelector(capture);
    if (scroller) scroller.scrollTop = scroller.scrollHeight;
    if (!mac) return;
    const barCentre = document.querySelector('.app-header').getBoundingClientRect().bottom / 2;
    ['#ff5f57', '#febc2e', '#28c840'].forEach((colour, i) => {
      const light = document.createElement('div');
      light.style.cssText = `position:fixed;z-index:2147483647;width:12px;height:12px;border-radius:50%;`
        + `left:${mac.lightsXPx + 4 + i * 20}px;top:${barCentre - 6}px;background:${colour}`;
      document.body.appendChild(light);
    });
  }, { css, html, into, capture, mac });
  await page.waitForTimeout(400);
  if (a.hover) await page.hover(a.hover);
  if (capture === 'viewport') await page.screenshot({ path: a.out });
  else await page.locator(capture).first().screenshot({ path: a.out });
  await browser.close();
})().catch((e) => {
  console.error(e);
  process.exit(1);
});
