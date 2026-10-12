// from Sources/Search/Fork/Canvas/CanvasFrames.swift:114 CanvasHost.frameReporter
(function () {
  if (window === window.top) return;
  var direct = false;
  try { direct = window.parent === window.top; } catch (_) {}
  if (!direct) return;
  var name = String(window.name || '');
  if (name.indexOf('copper-frame:') !== 0 || name.length > 200) return;
  if (location.protocol !== 'https:' && location.protocol !== 'http:') return;
  var handler = window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers./*INTERP: frameHandler*/;
  if (!handler) return;
  function post(message) {
    message.name = name;
    try { handler.postMessage(message); } catch (_) {}
  }
  function icon() {
    var links = document.querySelectorAll('link[rel~="icon"], link[rel="shortcut icon"], link[rel~="apple-touch-icon"]');
    var touch = '';
    for (var i = 0; i < links.length; i++) {
      var href = links[i].href || '';
      if (!/^https:/i.test(href) || href.length > 2048) continue;
      if (/apple-touch-icon/i.test(links[i].rel)) { touch = touch || href; continue; }
      return href;
    }
    if (touch) return touch;
    return location.protocol === 'https:' ? location.origin + '/favicon.ico' : '';
  }
  var last = '';
  function report() {
    var url = location.href;
    var title = String(document.title || '').slice(0, 500);
    var ic = document.head ? icon() : '';
    var key = url + '\n' + title + '\n' + ic;
    if (key === last) return;
    last = key;
    post({ kind: 'nav', url: url, title: title, icon: ic });
  }
  document.addEventListener('DOMContentLoaded', report);
  window.addEventListener('load', report);
  window.addEventListener('popstate', report);
  window.addEventListener('hashchange', report);
  if (window.navigation && typeof window.navigation.addEventListener === 'function') {
    window.navigation.addEventListener('navigatesuccess', function () { setTimeout(report, 0); });
  }
  // An app that changes its address or title without either event.
  setInterval(function () { if (document.visibilityState === 'visible') report(); }, 1000);
  window.addEventListener('keydown', function (e) {
    if (e.key !== 'Escape' || e.isComposing) return;
    var full = !!(document.fullscreenElement || document.webkitFullscreenElement);
    setTimeout(function () { if (!e.defaultPrevented && !full) post({ kind: 'escape' }); }, 0);
  });
  // A page that hides itself when it finds it is framed.
  function hidden(el) {
    if (!el) return false;
    var s = getComputedStyle(el);
    return s.display === 'none' || s.visibility === 'hidden' || Number(s.opacity) === 0;
  }
  window.addEventListener('load', function () {
    setTimeout(function () {
      if (!(hidden(document.documentElement) || hidden(document.body))) return;
      setTimeout(function () {
        if (hidden(document.documentElement) || hidden(document.body)) post({ kind: 'blank', url: location.href });
      }, 1500);
    }, 3000);
  });
})();
