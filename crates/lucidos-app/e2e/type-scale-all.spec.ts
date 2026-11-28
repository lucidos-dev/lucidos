import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp } from './helpers';
import { offenders, report } from './typeScaleWalk';

test.use({ viewport: { width: 1280, height: 900 } });

const ASSERTED_CONTROLS = ['input', 'textarea', 'button'];

test.describe('Type scale', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('the root font-size is not itself a body step', async ({ page }) => {
    await navigateToApp(page);
    const { rootPx, mdPx, xlPx } = await page.evaluate(() => {
      const s = getComputedStyle(document.documentElement);
      const px = (v: string) => {
        const probe = document.createElement('div');
        probe.style.fontSize = v;
        document.body.appendChild(probe);
        const out = parseFloat(getComputedStyle(probe).fontSize);
        probe.remove();
        return out;
      };
      return {
        rootPx: parseFloat(s.fontSize),
        mdPx: px(s.getPropertyValue('--font-size-md').trim()),
        xlPx: px(s.getPropertyValue('--font-size-xl').trim()),
      };
    });
    expect(rootPx).toBeCloseTo(xlPx, 1);
    expect(mdPx).toBeLessThan(rootPx);
  });

  test('body text falls to the body step, never to the root', async ({ page }) => {
    await navigateToApp(page);
    const { bodyPx, mdPx, probePx } = await page.evaluate(() => {
      const s = getComputedStyle(document.documentElement);
      const probe = document.createElement('div');
      probe.textContent = 'probe';
      document.body.appendChild(probe);
      const probePx = parseFloat(getComputedStyle(probe).fontSize);
      probe.remove();

      const sizer = document.createElement('div');
      sizer.style.fontSize = s.getPropertyValue('--font-size-md').trim();
      document.body.appendChild(sizer);
      const mdPx = parseFloat(getComputedStyle(sizer).fontSize);
      sizer.remove();

      return { bodyPx: parseFloat(getComputedStyle(document.body).fontSize), mdPx, probePx };
    });
    expect(bodyPx).toBeCloseTo(mdPx, 1);
    expect(probePx).toBeCloseTo(mdPx, 1);
  });

  test('an undeclared form control inherits the app font and size', async ({ page }) => {
    await navigateToApp(page);
    const results = await page.evaluate((tags: string[]) => {
      const s = getComputedStyle(document.documentElement);
      const expectFamily = s.getPropertyValue('--font-ui').trim().replace(/["']/g, '').toLowerCase();
      const sizer = document.createElement('div');
      sizer.style.fontSize = s.getPropertyValue('--font-size-md').trim();
      document.body.appendChild(sizer);
      const mdPx = parseFloat(getComputedStyle(sizer).fontSize);
      sizer.remove();

      const out: { tag: string; px: number; family: string }[] = [];
      for (const tag of tags) {
        const el = document.createElement(tag);
        document.body.appendChild(el);
        const cs = getComputedStyle(el);
        out.push({
          tag,
          px: parseFloat(cs.fontSize),
          family: cs.fontFamily.replace(/["']/g, '').toLowerCase(),
        });
        el.remove();
      }
      return { mdPx, expectFamily, out };
    }, ASSERTED_CONTROLS);

    for (const control of results.out) {
      expect(control.px, `<${control.tag}> font-size`).toBeCloseTo(results.mdPx, 1);
      expect(control.family, `<${control.tag}> font-family`).toBe(results.expectFamily);
    }
  });

  test('every visible text run in the thread view is on the scale', async ({ page }) => {
    await navigateToApp(page);
    const found = await offenders(page);
    expect(found, `off-scale text in the thread view:\n${report(found)}`).toEqual([]);
  });
});
