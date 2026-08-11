import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, openTriggersPanel } from './helpers';

/** The trigger group heading's two glyphs read at the same size.
 *
 *  Reported three times as the trash being small. The box was never the
 *  problem: both buttons are one `.icon-btn.row-icon`, 2.25rem holding a glyph
 *  at the box's nominal size (`--icon-glyph`, which defaults to
 *  `--icon-size-lg`; this pair takes the default, only the queued message's
 *  trash turns it down). What differs is how much of that glyph each icon
 *  actually paints. Measured here at a 20px box, the pencil's ink is 20.12 of
 *  its 24 viewBox units and the trash's is 16.00, so the trash rendered a fifth
 *  shorter inside an identical box.
 *
 *  `.icon-btn .trash-icon` in host-components.css corrects for that by the
 *  ratio, and takes the ratio back out of the stroke so the bigger glyph is not
 *  also a heavier one. Both halves are measurements of the artwork in
 *  `icons.tsx`, which is exactly why they are asserted against the real
 *  rendering rather than trusted: redraw either glyph without re-measuring and
 *  this fails instead of quietly shipping a mismatched pair again.
 *
 *  Runs on every project. The optics do not depend on the viewport, and the
 *  desktop run is the cheapest place to catch a redraw. */

const PREFIX = 'e2e-optics';

/** Rendered ink of a button's glyph, in CSS px: `getBBox()` is in the glyph's
 *  own user units, so scale it by how those units map to the rendered box.
 *  Stroke is excluded, which is not a choice: no DOM box includes it. */
type Ink = { inkHeight: number; inkWidth: number; box: number };

test.describe('Trigger group heading glyphs share one optical size', () => {
  test.afterEach(async ({ page }) => {
    try {
      const res = await page.request.get('/api/v1/trigger-groups');
      const body = await res.json();
      for (const g of (body.groups ?? []) as Array<{ id: string; name: string }>) {
        if (g.name?.startsWith(PREFIX)) await page.request.delete(`/api/v1/trigger-groups?id=${g.id}`);
      }
    } catch {
      /* best-effort cleanup */
    }
  });

  test('the trash lands the pencil\'s ink height, at the pencil\'s stroke weight', async ({ page }) => {
    await assertHealthy(page);
    const groupName = `${PREFIX}-${Date.now()}`;
    const created = await page.request.post('/api/v1/trigger-groups', { data: { name: groupName } });
    expect(created.ok(), 'failed to create the group over the API').toBe(true);

    await navigateToApp(page);
    await openTriggersPanel(page);

    const section = page.locator('.trigger-group-section').filter({
      has: page.locator('.trigger-group-name', { hasText: groupName }),
    });
    await expect(section.locator('.trigger-group-delete')).toBeVisible({ timeout: 10_000 });

    const measured = await page.evaluate(name => {
      const heading = Array.from(document.querySelectorAll('.trigger-group-header')).find(
        h => h.querySelector('.trigger-group-name')?.textContent === name,
      );
      const read = (sel: string) => {
        const svg = heading?.querySelector(`${sel} svg`) as SVGGraphicsElement | null;
        if (!svg) return null;
        const rect = svg.getBoundingClientRect();
        const vb = svg.viewBox.baseVal;
        const ink = svg.getBBox();
        // One user unit in CSS px. The viewBox is square and so is the box, so
        // one factor serves both axes.
        const unit = rect.width / vb.width;
        return {
          inkHeight: ink.height * unit,
          inkWidth: ink.width * unit,
          box: rect.width,
        };
      };
      return { pencil: read('.trigger-group-rename'), trash: read('.trigger-group-delete') };
    }, groupName);

    const pencil = measured.pencil as Ink | null;
    const trash = measured.trash as Ink | null;
    expect(pencil, 'no rename glyph found').not.toBeNull();
    expect(trash, 'no delete glyph found').not.toBeNull();

    // The property the user actually sees: two glyphs on one line, same height.
    // 6% covers sub-pixel layout and the two engines' arc flattening; the bug
    // this replaces was 21%.
    const heightRatio = trash!.inkHeight / pencil!.inkHeight;
    expect(
      Math.abs(heightRatio - 1),
      `trash ink ${trash!.inkHeight.toFixed(2)}px vs pencil ${pencil!.inkHeight.toFixed(2)}px`,
    ).toBeLessThan(0.06);

    // The other half of the correction, that the height was not bought with a
    // heavier line, is NOT asserted here: the applied stroke is not readable
    // from the DOM. Every box the DOM offers excludes it (`getBoundingClientRect`
    // on an SVG element is the geometry box, and `getBBox({stroke: true})` is
    // ignored by both engines), and reading the declaration instead tests the
    // serializer, which hands back an unresolved `calc()` on Chromium. It is
    // guarded structurally instead, by
    // src/styles/__tests__/trash-icon-optical-size.test.ts.

    // The grown glyph still fits the button it lives in, with room to spare.
    const buttonBox = (await section.locator('.trigger-group-delete').boundingBox())!;
    expect(trash!.box, 'the trash glyph outgrew its button').toBeLessThan(buttonBox.width);
  });
});
