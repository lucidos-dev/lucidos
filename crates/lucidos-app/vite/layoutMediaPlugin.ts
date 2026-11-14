/**
 * Expand `@media (--phone-layout)` and `@media (--desktop-layout)` into the
 * queries `src/utils/layoutMedia.ts` builds, in dev and in the build alike.
 *
 * A PostCSS plugin rather than a Vite transform hook, so it runs on every
 * stylesheet after Vite inlines `@import`s. Stylesheets served outside Vite
 * (`global/shared-components.css` in app frames) never pass through it, so
 * they must not use the names.
 */

import type { AtRule, PluginCreator } from 'postcss';
import { expandLayoutMedia } from '../src/utils/layoutMedia';

export const layoutMediaPlugin: PluginCreator<void> = () => ({
  postcssPlugin: 'lucidos-layout-media',
  AtRule: {
    media(rule: AtRule) {
      rule.params = expandLayoutMedia(rule.params);
    },
  },
});
layoutMediaPlugin.postcss = true;
