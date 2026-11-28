import { touchActivated } from '../../utils/tapGesture';
import { focusPromptNow } from './promptFocus';

export function composeHandlers(action: () => void, focusFn: () => void = focusPromptNow) {
  return touchActivated(() => {
    focusFn();
    action();
  });
}
