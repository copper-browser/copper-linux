// from Sources/Search/Swipe.swift:33 Swipe.calm
// user script: document start, main frame, page world.
function () {
  var sheet = document.createElement('style');
  sheet.id = 'office-calm';
  sheet.textContent = 'html, body { overscroll-behavior-y: none; }';
  (document.head || document.documentElement).appendChild(sheet);
}
