// from Sources/Search/ImageMenu.swift:32 ImageRelay.watch
// user script: document start, all frames, page world. Posts to the officeImages handler.
function () {
  if (window.__officeImages) return;
  window.__officeImages = true;
  document.addEventListener('contextmenu', function (e) {
    var el = e.target;
    while (el && el.tagName !== 'IMG') el = el.parentElement;
    if (!el || !el.currentSrc || el.naturalWidth < 2) return;
    e.preventDefault();
    window.webkit.messageHandlers.officeImages.postMessage({ src: el.currentSrc });
  }, true);
}
