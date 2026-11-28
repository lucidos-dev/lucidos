import { test, expect } from './fixtures';
import { apiRequest, assertHealthy, navigateToApp, openTriggersPanel } from './helpers';


const PREFIX = 'e2e-optics';

type Ink = { inkHeight: number; inkWidth: number; box: number };

test.describe('Trigger group heading glyphs share one optical size', () => {
  test.afterEach(async ({ page }) => {
    try {
      const res = await page.request.get('/api/v1/trigger-groups');
      const body = await res.json();
      for (const g of (body.groups ?? []) as Array<{ id: string; name: string }>) {
        if (g.name?.startsWith(PREFIX)) await apiRequest(page).delete(`/api/v1/trigger-groups?id=${g.id}`);
      }
    } catch {
    }
  });

  test('the trash lands the pencil\'s ink height, at the pencil\'s stroke weight', async ({ page }) => {
    await assertHealthy(page);
    const groupName = `${PREFIX}-${Date.now()}`;
    const created = await apiRequest(page).post('/api/v1/trigger-groups', { data: { name: groupName } });
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

    const heightRatio = trash!.inkHeight / pencil!.inkHeight;
    expect(
      Math.abs(heightRatio - 1),
      `trash ink ${trash!.inkHeight.toFixed(2)}px vs pencil ${pencil!.inkHeight.toFixed(2)}px`,
    ).toBeLessThan(0.06);


    const buttonBox = (await section.locator('.trigger-group-delete').boundingBox())!;
    expect(trash!.box, 'the trash glyph outgrew its button').toBeLessThan(buttonBox.width);
  });
});
