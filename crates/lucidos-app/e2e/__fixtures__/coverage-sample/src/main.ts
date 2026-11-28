import { greet } from './greet';
import { neverCalled } from './unused';

function start(): void {
  document.title = greet('coverage');
}

(window as unknown as { later: () => number }).later = neverCalled;
start();
