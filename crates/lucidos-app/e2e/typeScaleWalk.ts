export const SCALE_STEPS = [0.5625, 0.625, 0.6875, 0.75, 0.8125, 0.875, 1, 1.125, 1.25, 2.25];

export const EXEMPT_SUBTREES: { selector: string; why: string }[] = [
  {
    selector: '.boot-splash',
    why:
      'Fixed px and a fixed system-mono stack, both load-bearing. The gateway ' +
      'serves this same splash on the same URL while a workspace engine starts, ' +
      'by include_str!-ing the block out of index.html, and it has no bundle CSS ' +
      'and a different root font-size. A rem here would paint at two sizes and ' +
      'visibly jump when this document takes over. See index.html.',
  },
  {
    selector: '.scale-modal-overlay',
    why:
      'Pins a fixed 16px root and sizes its interior in em, so the scale PREVIEW ' +
      'does not resize while the user drags the slider that changes the real ' +
      'root. Kept although no walked view opens the modal: unlike the three that ' +
      'were removed, this one is a real fixed-root subtree, so the entry matches ' +
      'a live rule and the walk would flag it the moment a view opens it.',
  },
];

export interface Offender {
  where: string;
  fontSizePx: number;
  ratio: number;
  fontFamily: string;
  reason: 'off-scale size' | 'foreign font';
  text: string;
}

export async function offenders(page: import('@playwright/test').Page): Promise<Offender[]> {
  return page.evaluate(
    ({ steps, exemptSubtrees }) => {
      const rootStyle = getComputedStyle(document.documentElement);
      const rootPx = parseFloat(rootStyle.fontSize);
      const fontUi = rootStyle.getPropertyValue('--font-ui').trim();
      const fontMono = rootStyle.getPropertyValue('--font-mono').trim();

      const normaliseFamily = (s: string): string =>
        s.replace(/["']/g, '').replace(/\s+/g, ' ').trim().toLowerCase();
      const allowedFamilies = [fontUi, fontMono].map(normaliseFamily).filter(Boolean);

      const describe = (el: Element): string => {
        const parts: string[] = [];
        for (let node: Element | null = el; node && node !== document.body; node = node.parentElement) {
          const cls = node.className && typeof node.className === 'string'
            ? '.' + node.className.trim().split(/\s+/).join('.')
            : '';
          parts.unshift(node.tagName.toLowerCase() + cls);
          if (parts.length >= 3) break;
        }
        return parts.join(' > ');
      };

      const out: Offender[] = [];
      const seen = new Set<string>();

      for (const el of Array.from(document.querySelectorAll('*'))) {
        const tag = el.tagName.toLowerCase();
        if (tag === 'script' || tag === 'style' || tag === 'svg' || tag === 'path') continue;
        if (exemptSubtrees.some((sel: string) => el.closest(sel))) continue;

        const ownText = Array.from(el.childNodes)
          .filter(n => n.nodeType === Node.TEXT_NODE)
          .map(n => (n.textContent ?? '').trim())
          .join(' ')
          .trim();
        if (!ownText) continue;

        const rect = el.getBoundingClientRect();
        if (rect.width === 0 || rect.height === 0) continue;
        const style = getComputedStyle(el);
        if (style.visibility === 'hidden' || style.display === 'none') continue;

        const fontSizePx = parseFloat(style.fontSize);
        const ratio = fontSizePx / rootPx;
        const where = describe(el);
        const sample = ownText.slice(0, 40);

        const onScale = steps.some((s: number) => Math.abs(fontSizePx - s * rootPx) < 0.25);
        if (!onScale) {
          const key = `size:${where}:${fontSizePx}`;
          if (!seen.has(key)) {
            seen.add(key);
            out.push({
              where,
              fontSizePx,
              ratio: Math.round(ratio * 10000) / 10000,
              fontFamily: style.fontFamily,
              reason: 'off-scale size',
              text: sample,
            });
          }
        }

        const family = normaliseFamily(style.fontFamily);
        if (allowedFamilies.length > 0 && !allowedFamilies.includes(family)) {
          const key = `font:${where}:${family}`;
          if (!seen.has(key)) {
            seen.add(key);
            out.push({
              where,
              fontSizePx,
              ratio: Math.round(ratio * 10000) / 10000,
              fontFamily: style.fontFamily,
              reason: 'foreign font',
              text: sample,
            });
          }
        }
      }
      return out;
    },
    { steps: SCALE_STEPS, exemptSubtrees: EXEMPT_SUBTREES.map(e => e.selector) }
  );
}

export function report(found: Offender[]): string {
  return found
    .map(
      o =>
        `  [${o.reason}] ${o.where}\n` +
        `      ${o.fontSizePx}px = ${o.ratio}rem  font: ${o.fontFamily}\n` +
        `      text: ${JSON.stringify(o.text)}`
    )
    .join('\n');
}

