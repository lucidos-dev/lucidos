/**
 * The shared walk behind `type-scale.spec.ts` and
 * `type-scale-settings-desktop.spec.ts`: collect every visible run of text in
 * the live document and report the ones that are not on the type scale, or not
 * in one of the app's two fonts.
 *
 * This is the only check in the repo that resolves the CASCADE, and that is the
 * whole reason it exists. The recurring defect is a MISSING declaration, not a
 * wrong one: a surface that styles its padding and stops inherits from whatever
 * is above it, and no source scan can follow that. `base.css` supplies two
 * defaults that make such an omission harmless (`body` gets `--font-size-md`,
 * and form controls inherit family and size past the UA's `font` shorthand);
 * these specs are what prove they reach the pixels.
 *
 * Why the failure is always "too big" when it happens: `--font-size-xl` is
 * exactly `1rem`, and it is a SECTION HEADING. Anything that falls all the way
 * through to the root therefore lands a step and a half above body text. That
 * is what shipped in Settings > System > What's New, where the release notes
 * came out larger than the version heading they sat under.
 *
 * A module rather than a spec so the two callers share one definition of "on
 * the scale": a second copy would be free to drift into agreeing with whatever
 * the app happened to render.
 */
/**
 * The type scale from `styles/global/base.css`, as multiples of the root
 * font-size. Kept as multipliers rather than pixels so the assertion holds at
 * every ui-scale: the tokens are `rem`, so they all move with the root.
 *
 * Mirrored by `styles/__tests__/text-defaults-guard.test.ts`, which parses the
 * real `:root` block and fails if this list drifts from it.
 */
export const SCALE_STEPS = [0.5625, 0.625, 0.6875, 0.75, 0.8125, 0.875, 1, 1.125, 1.25, 2.25];

/**
 * Subtrees that are off the scale on purpose. A descendant of any of these is
 * skipped entirely, size and font both.
 *
 * **OBSERVED-MINIMAL, never preemptive.** Add an entry only when the walk has
 * actually flagged the subtree and the site turns out to be deliberate. A
 * speculative entry is worse than no entry: it silently stops checking whatever
 * it matches, and if the class is misremembered it also asserts something false
 * about the codebase while matching nothing. The first draft of this list had
 * three such entries, written from a grep of `em` VALUES without mapping them
 * back to their selectors: one named a class that does not exist anywhere, and
 * two named classes that are sized from tokens rather than in `em`.
 *
 * So: every entry below is one the walk hit. Each is also a claim that some
 * part of the app may ignore the type scale, which should cost an argument in
 * review. The cheap thing must stay "name a token".
 */
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

/** A text run we could not account for, described well enough to go fix it. */
export interface Offender {
  where: string;
  fontSizePx: number;
  /** `fontSizePx / rootPx`, the number to compare against SCALE_STEPS. */
  ratio: number;
  fontFamily: string;
  reason: 'off-scale size' | 'foreign font';
  text: string;
}

/**
 * Walk the live document and report every visible element with its own text
 * whose computed size is not a scale step, or whose font is neither the UI nor
 * the mono face.
 *
 * "Its own text" means a direct non-empty text-node child. Without that filter
 * every ancestor up to `<body>` reports the same run and the output is a tree
 * of duplicates rather than a list of sites.
 */
export async function offenders(page: import('@playwright/test').Page): Promise<Offender[]> {
  return page.evaluate(
    ({ steps, exemptSubtrees }) => {
      const rootStyle = getComputedStyle(document.documentElement);
      const rootPx = parseFloat(rootStyle.fontSize);
      // Resolve the two sanctioned stacks at runtime: --font-ui is a user
      // preference (Fira Code by default) that JS publishes onto <html>, so a
      // hardcoded expectation here would assert the wrong thing on a workspace
      // whose owner picked something else.
      const fontUi = rootStyle.getPropertyValue('--font-ui').trim();
      const fontMono = rootStyle.getPropertyValue('--font-mono').trim();

      /** A computed font-family string, normalised so quoting cannot fail a match. */
      const normaliseFamily = (s: string): string =>
        s.replace(/["']/g, '').replace(/\s+/g, ' ').trim().toLowerCase();
      const allowedFamilies = [fontUi, fontMono].map(normaliseFamily).filter(Boolean);

      /** A stable, human-actionable description of where a run of text lives. */
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

        // Only elements carrying their own text, so each run is reported once
        // at the element that actually sizes it.
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

        // A quarter-pixel of slack: a browser rounds a computed rem to a
        // fractional px, and 0.5625 * an odd root is not exact.
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

/** One offender per line, with the ratio, since the ratio is what names the step. */
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

