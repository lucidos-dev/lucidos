import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(resolve(here, '../ImagePopup.tsx'), 'utf-8');
const css = readFileSync(resolve(here, '../../../styles/components.css'), 'utf-8');

function rule(selector: string): string {
  const block = css.match(new RegExp(`^\\${selector}\\s*\\{[^}]*\\}`, 'm'));
  expect(block, `no ${selector} rule found`).not.toBeNull();
  return block![0];
}

describe('image popup: visible zoom controls', () => {
  it('renders a cluster of three: out, the level control, in', () => {
    expect(source).toMatch(/class="image-popup-zoom"/);
    expect(source).toMatch(/zoomApiRef\.current\?\.step\(-1\)/);
    expect(source).toMatch(/zoomApiRef\.current\?\.preset\(\)/);
    expect(source).toMatch(/zoomApiRef\.current\?\.step\(1\)/);
  });

  it('the level control reads out the zoom, rather than naming its own action', () => {
    expect(source, 'the readout must be the live percentage').toMatch(/\{level\.percent\}%/);
    expect(source, "a 'Fit' / '100%' label reports the destination, not the level")
      .not.toMatch(/'Fit' : '100%'/);
    // The action is still named, in the tooltip and the accessible name, and
    // named as a verb: "450%, actual size" would claim the level, not the act.
    expect(source).toMatch(/level\.atFit \? 'Zoom to actual size' : 'Fit to window'/);
    expect(source).toMatch(/level\.percent}%, \$\{level\.atFit \? 'zoom to actual size' : 'fit to window'/);
  });

  it('greys out an end the zoom has already reached', () => {
    expect(source).toMatch(/disabled=\{level\.atMin\}/);
    expect(source).toMatch(/disabled=\{level\.atMax\}/);
  });

  // An image fitting the screen it was captured on is already at 1:1, so the
  // fit / actual-size toggle has one place to be. Left live it is a dead
  // button, and its label still promises a zoom that cannot happen.
  it('greys the level control out where fit and actual size are one place', () => {
    expect(source).toMatch(/disabled=\{!level\.canPreset\}/);
    expect(source).toMatch(/canPreset: !atFit \|\| full <= 0 \|\| !sameScale\(full, fit\)/);
    expect(source, 'a disabled toggle must not name an action it cannot take')
      .toMatch(/level\.canPreset\s*\n\s*\? `\$\{level\.percent\}%, /);
  });

  it('the percentage is measured against the image, not against the fitted view', () => {
    expect(source).toMatch(/percent: zoomPercent\(scale, full, fit\)/);
  });

  // The reported bug: the popup counted CSS pixels, so a phone screenshot
  // filling the phone it came from read 33%. Every pixel of it sat on a screen
  // pixel, and the "actual size" offered from there was a threefold blow-up.
  it('counts physical screen pixels, so 100% is one image pixel per screen pixel', () => {
    expect(source).toMatch(/fullSizeScale\(layout\.imgW, img\.naturalWidth, screenPixelRatio\(\)\)/);
    expect(source).toMatch(/function screenPixelRatio\(\)/);
    expect(source, 'captured once, it goes stale on browser zoom and a display move')
      .toMatch(/return window\.devicePixelRatio \|\| 1;/);
  });

  it('takes the zoom keys, and leaves a modified chord to the browser', () => {
    expect(source).toMatch(/e\.metaKey \|\| e\.ctrlKey \|\| e\.altKey/);
    expect(source).toMatch(/e\.key === '\+' \|\| e\.key === '='/);
    expect(source).toMatch(/e\.key === '-'/);
    expect(source).toMatch(/e\.key === '0'/);
  });

  it('never writes the zoom state mid-pinch, which would re-render each frame', () => {
    expect(source).toMatch(/if \(!touchActiveRef\.current\) publishLevel/);
  });

  it('a dismiss on a zoomed image unzooms it, rather than doing nothing', () => {
    const onClose = source.match(/onClose=\{[\s\S]*?\n      \}\}/);
    expect(onClose, 'no onClose handler found on the popup Overlay').not.toBeNull();
    expect(onClose![0]).toMatch(/zoom\?\.isZoomed\(\).*zoom\.fit\(\)/s);
  });
});

describe('image popup: an image opens fitted to the window', () => {
  // The bug: an image smaller than the window sat at its own size, and the
  // control called that a fit while reading out 100%.
  it('rests at the scale that reaches the window edge, not at scale 1', () => {
    expect(source).toMatch(/function fitScale\(\)/);
    expect(source).toMatch(/fitToWindowScale\(layout\.containerW, layout\.containerH/);
    expect(source, 'a fit that hardcodes scale 1 leaves a small image small')
      .toMatch(/zoomRef\.current = \{ scale: fitScale\(\), tx: 0, ty: 0 \}/);
  });

  it('takes the fit again once the image has something to measure', () => {
    expect(source).toMatch(/onLoad=\{\(\) => zoomApiRef\.current\?\.refit\(\)\}/);
    expect(source, 'a late load must not overwrite a zoom the user chose')
      .toMatch(/refit: \(\) => \{ if \(fittedRef\.current\) zoomToFit\(\); \}/);
  });

  it('re-fits on a resize, and only re-clamps a chosen zoom', () => {
    const resize = source.match(/function handleResize\(\)[\s\S]*?\n    \}/);
    expect(resize, 'no handleResize found').not.toBeNull();
    expect(resize![0]).toMatch(/if \(fittedRef\.current\) \{\s*zoomToFit\(\);/);
    expect(resize![0]).toMatch(/clamp\(\);\s*applyZoom\(\);/);
  });

  it('measures "zoomed" against the fit, so a fitted image is not a zoomed one', () => {
    expect(source).toMatch(/function zoomedPastFit\(\)/);
    expect(source, 'scale > 1 stopped meaning zoomed the moment a fit could exceed 1')
      .not.toMatch(/zoomRef\.current\.scale > 1/);
  });

  // Layout is recovered by dividing the live transform out of a measured rect,
  // which holds only while the element and zoomRef agree. A slide still wearing
  // the fit of a previous visit measures several times its own size. The fit
  // computed from that reopens the image at nothing like a fit, and tracking
  // only the ZOOMED image missed exactly the upscaled fitted one.
  it('remembers every image it transformed, so navigation can clear it', () => {
    expect(source).toMatch(/transformedImgRef\.current = img;/);
    expect(source, 'a fitted image carries a transform too, and is not "zoomed"')
      .not.toMatch(/ImgRef\.current = past \?/);
    const reset = source.match(/useLayoutEffect\(\(\) => \{\s*zoomRef\.current = \{ scale: 1[\s\S]*?\}, \[state\?\.index\]\);/);
    expect(reset, 'no zoom-reset layout effect found').not.toBeNull();
    expect(reset![0]).toMatch(/transformedImgRef\.current/);
    expect(reset![0]).toMatch(/old\.style\.transform = ''/);
  });
});

describe('image popup: full size is always reachable', () => {
  it('widens both ends per image instead of capping at a constant', () => {
    expect(source).toMatch(/function zoomRange\(fit: number, full: number\)/);
    expect(source).toMatch(/min: zoomFloor\(fit, full\)/);
    expect(source).toMatch(/max: zoomCeiling\(MAX_ZOOM_PAST_FIT \* fit, full\)/);
  });

  // A tall screenshot's 1:1 sits above the fit, a small image's below it, and a
  // phone screenshot's on it. One fixed end always hides one of the three.
  it('has no fixed floor of 1, which hid 1:1 from a blown-up image', () => {
    expect(source, 'a fixed floor is what put actual size out of reach')
      .not.toMatch(/MIN_SCALE/);
  });

  it('passes that range to the wheel, the double tap and the pinch', () => {
    expect(source.match(/range\.min, range\.max/g) ?? []).toHaveLength(3);
  });
});

describe('image popup: the chrome is pinned to the viewport', () => {
  // .image-popup-content clips its children, so pinch-zoom stays inside the
  // box. Anything positioned against that box and sitting outside it is simply
  // not drawn: the close button spent that way off the top of the screen.
  it('close, counter and zoom cluster are all fixed', () => {
    for (const selector of ['.image-popup-close', '.image-popup-counter', '.image-popup-zoom']) {
      expect(rule(selector), `${selector} must not be positioned against the clipped box`)
        .toMatch(/position:\s*fixed/);
    }
  });

  it('the readout holds its width and lines its digits up', () => {
    expect(rule('.image-popup-zoom-level')).toMatch(/font-variant-numeric:\s*tabular-nums/);
  });

  it('a control with nowhere left to go looks like it', () => {
    expect(rule('.image-popup-zoom-btn:disabled')).toMatch(/opacity:/);
    expect(css, 'a disabled control must not light up under the pointer')
      .toMatch(/\.image-popup-zoom-btn:not\(:disabled\):hover/);
  });
});
