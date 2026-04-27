import { setSplitRatio, DEFAULT_SPLIT_RATIO } from './splitHelpers';

export function resolveHeaderDblClick({ clickedThreadSide, ratio }: { clickedThreadSide: boolean; ratio: number }) {
  if (clickedThreadSide) {
    setSplitRatio(ratio >= 1 ? DEFAULT_SPLIT_RATIO : 1);
  } else {
    setSplitRatio(ratio === 0 ? DEFAULT_SPLIT_RATIO : 0);
  }
}
