/**
 * The shared walk behind `focus-ring-not-clipped.spec.ts`: focus every visible
 * control in a scope the way the keyboard would, and report each focus ring an
 * ancestor cuts.
 *
 * The ring (`--focus-ring`) is an outward `box-shadow`, and a shadow adds
 * nothing to a box's scrollable overflow. So a scrolling or clipping ancestor
 * cuts it wherever the control sits flush with that ancestor's edge. No source
 * scan can tell "flush", so this runs in the browser against the resolved
 * cascade. The fix is ring room at the clipping box
 * (`.claude/rules/frontend-css.md`).
 */
import type { Page } from '@playwright/test';

/** One cut ring: who draws it, what cuts it, and on which edges. */
export interface ClippedRing {
  control: string;
  ringOwner: string;
  clip: string;
  edges: string[];
}

export interface RingWalk {
  clipped: ClippedRing[];
  /** Controls checked. A walk that checked none proves nothing. */
  checked: number;
  /** Controls that never matched `:focus-visible`, so their ring went unseen. */
  notFocusVisible: string[];
}

/** Every control inside `scope` (a selector, default the whole document). */
export async function walkFocusRings(page: Page, scope = 'body'): Promise<RingWalk> {
  // Keyboard modality, so a programmatic focus matches :focus-visible.
  await page.keyboard.press('Shift');
  return page.evaluate(async (scopeSelector) => {
    // A surface still fading in reads as invisible, so let entrances finish.
    // A spinner never does, so infinite animations are left running.
    await Promise.all(document.getAnimations()
      .filter(a => a.effect?.getComputedTiming().iterations !== Infinity)
      .map(a => a.finished.catch(() => undefined)));
    const FOCUSABLE = [
      'a[href]', 'button:not([disabled])', 'input:not([type="hidden"]):not([disabled])',
      'textarea:not([disabled])', 'select:not([disabled])', 'summary',
      '[tabindex]:not([tabindex="-1"])', '[contenteditable="true"]',
    ].join(',');
    const SUBPIXEL = 0.5;
    type Sides = { top: number; right: number; bottom: number; left: number };
    type Box = { xmin: number; xmax: number; ymin: number; ymax: number };
    type Cut = { clip: string; edges: string[] };

    const describe = (el: Element): string => {
      const cls = [...el.classList].slice(0, 3).map(c => `.${c}`).join('');
      const label = el.getAttribute('aria-label') ?? el.getAttribute('data-role')
        ?? (el.textContent ?? '').trim().slice(0, 30);
      return `${el.tagName.toLowerCase()}${cls}${label ? ` "${label}"` : ''}`;
    };

    // Alpha 0 only: `rgba(…, 0)` or `/ 0)`, never an opaque `rgb(…, 0)`.
    const transparent = (color: string): boolean =>
      color === 'transparent' || /^rgba\([^)]*,\s*0\)$|\/\s*0\)$/.test(color.trim());

    // How far past the border box each painted ring reaches, per side.
    const reach = (el: Element): Sides => {
      const cs = getComputedStyle(el);
      const out: Sides = { top: 0, right: 0, bottom: 0, left: 0 };
      const grow = (top: number, right: number, bottom: number, left: number) => {
        out.top = Math.max(out.top, top);
        out.right = Math.max(out.right, right);
        out.bottom = Math.max(out.bottom, bottom);
        out.left = Math.max(out.left, left);
      };
      if (cs.boxShadow && cs.boxShadow !== 'none') {
        // Split on top-level commas; colour functions carry commas inside.
        let depth = 0;
        let start = 0;
        const parts: string[] = [];
        for (let i = 0; i < cs.boxShadow.length; i++) {
          const ch = cs.boxShadow[i];
          if (ch === '(') depth++;
          else if (ch === ')') depth--;
          else if (ch === ',' && depth === 0) {
            parts.push(cs.boxShadow.slice(start, i));
            start = i + 1;
          }
        }
        parts.push(cs.boxShadow.slice(start));
        for (const part of parts) {
          if (/\binset\b/.test(part)) continue;
          const color = /(rgba?|color|oklch|oklab|lab|lch|hsla?)\([^)]*\)/.exec(part)?.[0] ?? '';
          if (color && transparent(color)) continue;
          const nums = [...part.replace(color, '').matchAll(/(-?[\d.]+)px/g)].map(m => parseFloat(m[1]));
          const [x = 0, y = 0, blur = 0, spread = 0] = nums;
          const r = spread + blur;
          grow(r - y, r + x, r + y, r - x);
        }
      }
      if (cs.outlineStyle !== 'none' && !transparent(cs.outlineColor)) {
        const r = parseFloat(cs.outlineWidth) + parseFloat(cs.outlineOffset);
        grow(r, r, r, r);
      }
      return out;
    };

    // The elements a focus can repaint: the control, the frames around it
    // (`:focus-within`, `:has(:focus-visible)`), and a drawn sibling such as
    // a toggle's slider.
    const ringCandidates = (el: Element): Element[] => {
      const out: Element[] = [el];
      let a = el.parentElement;
      for (let i = 0; i < 3 && a; i++, a = a.parentElement) out.push(a);
      if (el.nextElementSibling) out.push(el.nextElementSibling);
      return out;
    };

    const makesCbForFixed = (cs: CSSStyleDeclaration): boolean =>
      cs.transform !== 'none' || cs.filter !== 'none' || cs.perspective !== 'none'
      || cs.backdropFilter !== 'none' || /paint|layout|strict|content/.test(cs.contain)
      || /transform|filter|perspective/.test(cs.willChange);
    const makesCbForAbsolute = (cs: CSSStyleDeclaration): boolean =>
      cs.position !== 'static' || makesCbForFixed(cs);

    // The box each ancestor lets paint, or null when it lets everything
    // through. A scroller lets through its whole scrollable area, which a
    // shadow never extends. A `clip` box lets through only its padding box.
    const overflowBox = (a: Element, cs: CSSStyleDeclaration): Box | null => {
      const r = a.getBoundingClientRect();
      const left = r.left + a.clientLeft;
      const top = r.top + a.clientTop;
      const box: Box = { xmin: -Infinity, xmax: Infinity, ymin: -Infinity, ymax: Infinity };
      let clips = false;
      if (cs.overflowX === 'clip') {
        box.xmin = left; box.xmax = left + a.clientWidth; clips = true;
      } else if (cs.overflowX !== 'visible') {
        box.xmin = left - a.scrollLeft; box.xmax = box.xmin + a.scrollWidth; clips = true;
      }
      if (cs.overflowY === 'clip') {
        box.ymin = top; box.ymax = top + a.clientHeight; clips = true;
      } else if (cs.overflowY !== 'visible') {
        box.ymin = top - a.scrollTop; box.ymax = box.ymin + a.scrollHeight; clips = true;
      }
      return clips ? box : null;
    };
    const clipPathBox = (a: Element, cs: CSSStyleDeclaration): Box | null => {
      const m = /^inset\(([^)]*?)(?:\s+round[^)]*)?\)$/.exec(cs.clipPath);
      if (!m) return null;
      const vals = m[1].trim().split(/\s+/);
      if (!vals.every(v => /^-?[\d.]+(px|%)$/.test(v))) return null;
      const r = a.getBoundingClientRect();
      // A percentage resolves against the box's height for top and bottom,
      // and its width for the sides.
      const px = (v: string, side: number) => v.endsWith('%') ? parseFloat(v) / 100 * side : parseFloat(v);
      const [tv, rv = tv, bv = tv, lv = rv] = vals;
      const [t, rt, b, l] = [px(tv, r.height), px(rv, r.width), px(bv, r.height), px(lv, r.width)];
      return { xmin: r.left + l, xmax: r.right - rt, ymin: r.top + t, ymax: r.bottom - b };
    };

    const cutsOf = (owner: Element, ring: Sides): Cut[] => {
      const r = owner.getBoundingClientRect();
      const ringBox: Box = {
        xmin: r.left - ring.left, xmax: r.right + ring.right,
        ymin: r.top - ring.top, ymax: r.bottom + ring.bottom,
      };
      // The owner's own clip-path shapes its ring on purpose, as a full-height
      // bar keeps only its sides.
      const own = clipPathBox(owner, getComputedStyle(owner));
      if (own) {
        ringBox.xmin = Math.max(ringBox.xmin, own.xmin);
        ringBox.xmax = Math.min(ringBox.xmax, own.xmax);
        ringBox.ymin = Math.max(ringBox.ymin, own.ymin);
        ringBox.ymax = Math.min(ringBox.ymax, own.ymax);
      }
      const found: Cut[] = [];
      const check = (a: Element, box: Box | null) => {
        if (!box) return;
        const edges: string[] = [];
        if (ring.top > 0 && ringBox.ymin < box.ymin - SUBPIXEL) edges.push('top');
        if (ring.right > 0 && ringBox.xmax > box.xmax + SUBPIXEL) edges.push('right');
        if (ring.bottom > 0 && ringBox.ymax > box.ymax + SUBPIXEL) edges.push('bottom');
        if (ring.left > 0 && ringBox.xmin < box.xmin - SUBPIXEL) edges.push('left');
        if (edges.length) found.push({ clip: describe(a), edges });
      };
      let position = getComputedStyle(owner).position;
      for (let a = owner.parentElement; a && a !== document.body; a = a.parentElement) {
        const cs = getComputedStyle(a);
        // A clip-path clips every descendant; overflow clips only the
        // containing-block chain.
        check(a, clipPathBox(a, cs));
        if (position === 'fixed' && !makesCbForFixed(cs)) continue;
        if (position === 'absolute' && !makesCbForAbsolute(cs)) continue;
        check(a, overflowBox(a, cs));
        // A scroller can bring the ring into its own view, so what clips the
        // scroller further out is a question about the scroller.
        const scrolls = (v: string) => v !== 'visible' && v !== 'clip';
        if (scrolls(cs.overflowX) || scrolls(cs.overflowY)) break;
        position = cs.position;
      }
      return found;
    };

    const scopeEl = document.querySelector(scopeSelector);
    if (!scopeEl) throw new Error(`no element matches ${scopeSelector}`);
    const controls = [...scopeEl.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(el =>
      !el.closest('[inert]')
      && el.getClientRects().length > 0
      && el.checkVisibility({ opacityProperty: true, visibilityProperty: true }));

    const noTransitions = document.createElement('style');
    noTransitions.textContent = '*, *::before, *::after { transition: none !important; }';
    document.head.append(noTransitions);
    const restore = document.activeElement as HTMLElement | null;

    const clipped: ClippedRing[] = [];
    const notFocusVisible: string[] = [];
    // A text field that is focused already goes first and keeps its focus: some
    // close on blur, like the device rename. With no resting state to compare,
    // only its own ring counts. Anything else is refocused the keyboard's way.
    const isTextField = (el: Element | null) => !!el?.matches('input, textarea, [contenteditable="true"]');
    const keep = isTextField(restore) ? restore : null;
    const focusedFirst = controls.filter(el => el === keep);
    for (const el of [...focusedFirst, ...controls.filter(el => el !== keep)]) {
      if (!el.isConnected) continue;
      const alreadyFocused = el === keep && el === document.activeElement;
      if (!alreadyFocused) (document.activeElement as HTMLElement | null)?.blur?.();
      const candidates = alreadyFocused ? [el] : ringCandidates(el);
      const atRest = candidates.map(c => alreadyFocused
        ? { top: 0, right: 0, bottom: 0, left: 0 }
        : reach(c));
      el.focus({ preventScroll: true, focusVisible: true } as FocusOptions);
      if (document.activeElement !== el) continue;
      if (!el.matches(':focus-visible')) {
        notFocusVisible.push(describe(el));
        continue;
      }
      candidates.forEach((owner, i) => {
        const now = reach(owner);
        const rest = atRest[i];
        const grew = now.top > rest.top || now.right > rest.right
          || now.bottom > rest.bottom || now.left > rest.left;
        if (!grew) return;
        for (const cut of cutsOf(owner, now)) {
          clipped.push({ control: describe(el), ringOwner: describe(owner), ...cut });
        }
      });
    }

    (document.activeElement as HTMLElement | null)?.blur?.();
    restore?.focus?.({ preventScroll: true });
    noTransitions.remove();
    return { clipped, checked: controls.length, notFocusVisible };
  }, scope);
}

/** One line per cut ring, for an assertion message. */
export function formatClipped(clipped: ClippedRing[]): string {
  return clipped
    .map(c => `${c.control}${c.ringOwner === c.control ? '' : ` (ring on ${c.ringOwner})`} cut ${c.edges.join('+')} by ${c.clip}`)
    .join('\n');
}
