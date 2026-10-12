// from Sources/Search/Fork/Canvas/CanvasLean.swift:78 CanvasHost.typing
// user script: document start, main frame, page world (the lean board). Posts to the officeForms handler.
function () {
  if (window.__copperBoardKeys) return;
  window.__copperBoardKeys = true;
  function editable(el) {
    if (!el) return false;
    var tag = (el.tagName || '').toLowerCase();
    if (tag === 'textarea' || el.isContentEditable === true) return true;
    if (tag !== 'input') return false;
    var kind = (el.type || 'text').toLowerCase();
    return ['text', 'search', 'email', 'url', 'tel', 'password', 'number'].indexOf(kind) >= 0;
  }
  var was = null;
  function tell() {
    var now = editable(document.activeElement);
    if (now === was) return;
    was = now;
    window.webkit.messageHandlers.officeForms.postMessage({ kind: 'focus', typing: now, rect: null, field: null, hint: '' });
  }
  document.addEventListener('focusin', tell, true);
  document.addEventListener('focusout', function () { setTimeout(tell, 0); }, true);
}
