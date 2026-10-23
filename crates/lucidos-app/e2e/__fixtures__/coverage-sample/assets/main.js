"use strict";
(() => {
  // e2e/__fixtures__/coverage-sample/src/greet.ts
  function greet(name) {
    return `hello ${name}`;
  }

  // e2e/__fixtures__/coverage-sample/src/unused.ts
  function neverCalled() {
    return 42;
  }

  // e2e/__fixtures__/coverage-sample/src/main.ts
  function start() {
    document.title = greet("coverage");
  }
  window.later = neverCalled;
  start();
})();
