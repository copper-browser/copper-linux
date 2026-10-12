// from Sources/Search/Float.swift:490 Isolate.on
(function () {
  var videos = document.querySelectorAll('video');
  var best = null, area = 0;
  for (var i = 0; i < videos.length; i++) {
    var v = videos[i];
    if (v.paused || v.ended || v.readyState < 2) continue;
    var box = v.getBoundingClientRect();
    if (box.width * box.height >= area) { area = box.width * box.height; best = v; }
  }
  if (!best) return 'none';

  best.setAttribute('data-office-float', '');
  var sheet = document.getElementById('office-float');
  if (!sheet) {
    sheet = document.createElement('style');
    sheet.id = 'office-float';
    (document.head || document.documentElement).appendChild(sheet);
  }
  sheet.textContent = [
    'html.office-floating, html.office-floating body {',
    'background:#000 !important; overflow:hidden !important; margin:0 !important}',
    'html.office-floating body > * { visibility:hidden !important }',
    'html.office-floating [data-office-float] {',
    'visibility:visible !important; position:fixed !important;',
    'left:0 !important; top:0 !important; right:0 !important; bottom:0 !important;',
    'width:100vw !important; height:100vh !important;',
    'max-width:none !important; max-height:none !important;',
    'object-fit:contain !important; z-index:2147483647 !important}',
    // Fixed or not, the video is still cut to the box of any ancestor
    // that clips — YouTube's player does — and in a window this small
    // that box sits partly or wholly off screen, more so on a page that
    // was scrolled. That was the black window.
    'html.office-floating body :has([data-office-float]) {',
    'overflow:visible !important}',
    // The player's own controls would sit under ours, and two sets of
    // buttons on one small window is one set too many.
    'html.office-floating [data-office-float]::-webkit-media-controls {',
    'display:none !important}'
  ].join('');
  document.documentElement.classList.add('office-floating');

  // The mark has to be defended.
  //
  // Everything but the marked element is hidden, so the moment a player
  // rebuilds its DOM — and they all do, on a quality change, an ad break,
  // a React re-render — the mark goes with the old element and the window
  // turns pure black while still holding a perfectly live page. That is the
  // black rectangle, and it is not an orphaned window at all.
  //
  // So the mark is put back on whatever is playing now, four times a
  // second, for as long as the page is out.
  clearInterval(window.__officeFloatWatch);
  window.__officeFloatWatch = setInterval(function () {
    if (document.querySelector('[data-office-float]')) return;
    var again = null, most = 0;
    var all = document.querySelectorAll('video');
    for (var j = 0; j < all.length; j++) {
      var one = all[j];
      if (one.paused || one.ended || one.readyState < 2) continue;
      var shape = one.getBoundingClientRect();
      if (shape.width * shape.height >= most) {
        most = shape.width * shape.height;
        again = one;
      }
    }
    if (again) again.setAttribute('data-office-float', '');
  }, 250);

  return 'floating';
})();
