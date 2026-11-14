import { describe, expect, it } from 'vitest';
import fixture from '../../../lucidos-engine/src/engine/widget_embeds.fixture.json';
import { scanWidgetEmbeds, withoutWidgetEmbeds } from './widgetEmbed';

interface FixtureEmbed {
  app_id?: string;
  label?: string | null;
  params?: Record<string, unknown> | null;
  error?: boolean;
}

interface FixtureCase {
  markdown: string;
  embeds: FixtureEmbed[];
  without_embeds: string | null;
}

// The engine's `widget_embed.rs` reads this same fixture (ADR 0415).
describe('the shared widget embed fixture', () => {
  for (const c of fixture as FixtureCase[]) {
    it(`parses ${c.markdown}`, () => {
      const matches = scanWidgetEmbeds(c.markdown);
      expect(matches).toHaveLength(c.embeds.length);
      matches.forEach((m, i) => {
        const want = c.embeds[i];
        if (want.error) {
          expect(m.embed.ok).toBe(false);
          return;
        }
        expect(m.embed.ok).toBe(true);
        if (!m.embed.ok) return;
        expect(m.embed.value.app_id).toBe(want.app_id);
        expect(m.embed.value.label ?? null).toBe(want.label ?? null);
        expect(m.embed.value.params ?? null).toEqual(want.params ?? null);
      });
      expect(withoutWidgetEmbeds(c.markdown, matches)).toBe(c.without_embeds);
    });
  }
});
