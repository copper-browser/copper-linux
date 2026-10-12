// Copper console capture (PageOptions::capture_console). Runs in the page
// world at document start and forwards console.* calls and uncaught errors to
// the __copperConsole message handler. References are taken before page
// script runs, so later page changes to JSON/Array/webkit do not affect it.
(() => {
  const handlers = window.webkit && window.webkit.messageHandlers;
  const handler = handlers && handlers.__copperConsole;
  if (!handler || window.__copperConsoleHooked) return;
  Object.defineProperty(window, '__copperConsoleHooked', { value: true });
  const post = handler.postMessage.bind(handler);
  const stringify = JSON.stringify;
  const map = Array.prototype.map;
  const fmt = (a) => {
    if (typeof a === 'string') return a;
    if (a instanceof Error) return String(a.stack || a);
    try {
      const s = stringify(a);
      return s === undefined ? String(a) : s;
    } catch (_) {
      return String(a);
    }
  };
  const send = (level, args) => {
    try {
      post({ level, message: map.call(args, fmt).join(' ') });
    } catch (_) {}
  };
  for (const level of ['log', 'info', 'warn', 'error', 'debug']) {
    const orig = console[level];
    if (typeof orig !== 'function') continue;
    console[level] = function (...args) {
      send(level, args);
      return orig.apply(this, args);
    };
  }
  addEventListener('error', (e) =>
    send('error', [`Uncaught ${e.message} (${e.filename}:${e.lineno}:${e.colno})`]),
  );
  addEventListener('unhandledrejection', (e) =>
    send('error', ['Unhandled promise rejection: ' + fmt(e.reason)]),
  );
})();
