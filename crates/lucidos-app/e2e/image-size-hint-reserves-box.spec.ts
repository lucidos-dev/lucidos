import { test, expect } from './fixtures';
import type { Page } from '@playwright/test';
import { navigateToApp, assertHealthy } from './helpers';

/**
 * A picture with an image size hint holds its loaded box before its bytes
 * arrive. So a question card or reply does not grow when the picture lands.
 *
 * This is a layout property, so only a browser can see it. The markup matches
 * what `renderMarkdown` emits for `![alt](x.png#WxH)`, pinned in
 * src/utils/renderMarkdown.test.ts. It is injected into real surface hosts so
 * the live app cascade applies, as markdown-table-columns.spec.ts does.
 */

/** Small, taller than the 24rem cap, and wider than the pane. */
const SIZES: Array<[number, number]> = [[200, 100], [400, 1200], [3000, 300]];

/** Two pane widths and two UI scales (root font sizes; '' is the default). */
const HOST_WIDTHS = [360, 900];
const ROOT_FONT_SIZES = ['', '22px'];

/** A route that never answers, so a picture pointed at it stays unloaded. */
const PENDING = '/__never__/pending.png';

const SURFACES = ['response-content markdown-content', 'question-body'];

interface Box { w: number; h: number }
interface Measured { label: string; before: Box; after: Box; unhinted: Box }

/** Mounts a hinted picture of each size against the pending route, measures
 *  it, then loads the real bytes into it and into an unhinted copy.
 *  `swapHint` gives each picture a wrong hint on purpose. */
function measure(page: Page, hostClass: string, swapHint: boolean): Promise<Measured[]> {
  return page.evaluate(async ({ sizes, widths, fonts, pending, hostClass, swapHint }) => {
    const picture = async (w: number, h: number) => {
      const canvas = document.createElement('canvas');
      canvas.width = w;
      canvas.height = h;
      const blob = await new Promise<Blob>((ok) => canvas.toBlob((b) => ok(b!), 'image/png'));
      return URL.createObjectURL(blob);
    };
    const box = (img: HTMLImageElement) => {
      const r = img.getBoundingClientRect();
      return { w: Math.round(r.width), h: Math.round(r.height) };
    };

    const out = [];
    const root = document.documentElement;
    const restoreFont = root.style.fontSize;
    for (const font of fonts) {
      root.style.fontSize = font;
      for (const width of widths) {
        const host = document.createElement('div');
        host.className = hostClass;
        host.style.width = `${width}px`;
        document.body.appendChild(host);
        const mount = (src: string, hint: [number, number] | null) => {
          const wrapper = document.createElement('span');
          wrapper.className = 'image-scroll-wrapper';
          const img = document.createElement('img');
          if (hint) {
            img.setAttribute('data-size-hint', '');
            img.style.setProperty('--hint-w', String(hint[0]));
            img.style.setProperty('--hint-h', String(hint[1]));
          }
          img.src = src;
          wrapper.append(img);
          host.append(wrapper);
          return img;
        };
        for (const [w, h] of sizes) {
          const src = await picture(w, h);
          const hint: [number, number] = swapHint ? [h, w] : [w, h];
          const hinted = mount(`${pending}?${w}x${h}`, hint);
          const before = box(hinted);
          hinted.src = src;
          await hinted.decode();
          const after = box(hinted);
          const plain = mount(src, null);
          await plain.decode();
          out.push({ label: `${w}x${h} in ${width}px, font ${font || 'default'}`, before, after, unhinted: box(plain) });
        }
        host.remove();
      }
    }
    root.style.fontSize = restoreFont;
    return out;
  }, { sizes: SIZES, widths: HOST_WIDTHS, fonts: ROOT_FONT_SIZES, pending: PENDING, hostClass, swapHint });
}

test.describe('Image size hint', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await page.route(`**${PENDING}*`, () => new Promise(() => {}));
    await navigateToApp(page);
  });

  for (const surface of SURFACES) {
    const name = surface.split(' ')[0];

    test(`the box before load equals the loaded box in .${name}`, async ({ page }) => {
      for (const r of await measure(page, surface, false)) {
        expect(r.before.h, `${r.label} reserves a height`).toBeGreaterThan(0);
        expect(r.before, `${r.label} before load`).toEqual(r.after);
        expect(r.after, `${r.label} matches an unhinted copy`).toEqual(r.unhinted);
      }
    });

    test(`a wrong hint never stretches the loaded picture in .${name}`, async ({ page }) => {
      for (const r of await measure(page, surface, true)) {
        // Once loaded, the picture's own ratio sets the height for the width
        // the hint gave it, never above the cap an unhinted copy shows.
        const ownRatioHeight = Math.round(r.after.w * r.unhinted.h / r.unhinted.w);
        expect(r.after.h, `${r.label} keeps its own ratio`).toBeLessThanOrEqual(
          Math.max(ownRatioHeight, r.unhinted.h) + 1,
        );
      }
      const fit = await page.evaluate(() => {
        const img = document.createElement('img');
        img.setAttribute('data-size-hint', '');
        const wrapper = document.createElement('span');
        wrapper.className = 'image-scroll-wrapper';
        wrapper.append(img);
        document.body.append(wrapper);
        const value = getComputedStyle(img).objectFit;
        wrapper.remove();
        return value;
      });
      expect(fit).toBe('contain');
    });
  }
});
