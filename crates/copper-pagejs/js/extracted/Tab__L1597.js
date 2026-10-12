// from Sources/Search/Tab.swift:1597 ScrollRelay.script
(function () {
  var waiting = false;
  function tell() {
    var root = document.documentElement;
    var y = window.scrollY || root.scrollTop || 0;
    var ceiling = Math.max(1, (root.scrollHeight || 0) - window.innerHeight);
    window.webkit.messageHandlers./*INTERP: name*/.postMessage({ y: y, max: ceiling });
  }
  window.addEventListener('scroll', function () {
    if (waiting) return;
    waiting = true;
    requestAnimationFrame(function () { waiting = false; tell(); });
  }, { passive: true });
  tell();
})();
