import { lazyComponent } from '../../utils/lazyComponent';
import { prefetchWhenIdle } from '../../utils/idlePrefetch';

export const WidgetMenuItems = lazyComponent(() => import('./WidgetMenu').then((m) => m.WidgetMenuItems));
prefetchWhenIdle(WidgetMenuItems);
