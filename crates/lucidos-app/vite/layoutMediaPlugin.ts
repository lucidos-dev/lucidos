
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
