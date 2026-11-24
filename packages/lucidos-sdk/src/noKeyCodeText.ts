import { isKeyCodeTextInsertion } from './textEntry';

let installed = false;

export function installNoKeyCodeText(): void {
  if (installed) return;
  installed = true;
  document.addEventListener('beforeinput', (e) => {
    if (isKeyCodeTextInsertion(e.inputType, e.data)) e.preventDefault();
  }, { capture: true });
}
