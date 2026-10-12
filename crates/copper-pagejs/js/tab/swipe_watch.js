// from Sources/Search/Swipe.swift:45 Swipe.watch
// user script: document start, all frames, page world. Posts to the officeScroll handler.
function () {
  if (window.__officeSwipe) return;
  window.__officeSwipe = true;

  var was = null, said = 0;

  function rootCanScroll() {
    var html = getComputedStyle(document.documentElement).overflowX;
    var body = document.body ? getComputedStyle(document.body).overflowX : 'visible';
    var effective = html === 'visible' ? body : html;
    return effective !== 'hidden' && effective !== 'clip';
  }

  function taken(e) {
    var el = e.target;
    if (el && el.nodeType !== 1) el = el.parentElement;
    while (el) {
      var root = el === document.documentElement || el === document.body;
      var can, left, max;
      if (root) {
        can = rootCanScroll();
        left = window.scrollX || 0;
        max = document.documentElement.scrollWidth - window.innerWidth;
      } else {
        var ox = getComputedStyle(el).overflowX;
        can = ox === 'auto' || ox === 'scroll';
        left = el.scrollLeft;
        max = el.scrollWidth - el.clientWidth;
      }
      if (can && max > 1) {
        if (e.deltaX > 0 ? left < max - 1 : left > 1) return true;
      }
      el = el.parentElement;
    }
    return false;
  }

  window.addEventListener('wheel', function (e) {
    if (Math.abs(e.deltaX) <= Math.abs(e.deltaY)) return;
    var t = taken(e), now = Date.now();
    if (t === was && now - said < 100) return;
    was = t; said = now;
    window.webkit.messageHandlers.officeScroll.postMessage({ side: t ? 'taken' : 'free' });
  }, { passive: true, capture: true });
}
