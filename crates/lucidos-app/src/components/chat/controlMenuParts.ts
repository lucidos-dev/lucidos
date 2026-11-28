
import { chunkGroup, lazyComponent } from '../../utils/lazyComponent';
import { prefetchWhenIdle } from '../../utils/idlePrefetch';

const parts = () => import('./controlMenuPanelParts');

export const ControlFilter = lazyComponent(() => parts().then((m) => m.ControlFilter));
export const ControlOptionList = lazyComponent(() => parts().then((m) => m.ControlOptionList));
export const RungPicker = lazyComponent(() => parts().then((m) => m.RungPicker));
export const FrontendPreviewSection = lazyComponent(() => parts().then((m) => m.FrontendPreviewSection));

export const controlMenuParts = chunkGroup(ControlFilter, ControlOptionList, RungPicker, FrontendPreviewSection);
prefetchWhenIdle(controlMenuParts);
