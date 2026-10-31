import { lazyComponent } from '../../utils/lazyComponent';
import { prefetchWhenIdle } from '../../utils/idlePrefetch';

/** The widget menus' rows, fetched once the boot splash lifts (ADR 0436). Each
 *  menu opens once its rows are in memory, through `OverflowMenu`'s
 *  `itemsChunk`. */
export const WidgetMenuItems = lazyComponent(() => import('./WidgetMenu').then((m) => m.WidgetMenuItems));
prefetchWhenIdle(WidgetMenuItems);
