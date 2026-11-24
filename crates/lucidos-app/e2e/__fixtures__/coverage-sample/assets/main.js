"use strict";
(() => {
  function greet(name) {
    return `hello ${name}`;
  }

  function neverCalled() {
    return 42;
  }

  function start() {
    document.title = greet("coverage");
  }
  window.later = neverCalled;
  start();
})();
