// from Sources/Search/Float.swift:597 Isolate.off
(function () {
  // The engine may have put the video in its own floating window as well —
  // some players ask for that themselves. Leaving one and not the other
  // leaves you with two.
  try {
    var out = document.querySelector('video[data-office-float]')
      || document.querySelector('video');
    if (out) {
      if (out.webkitPresentationMode === 'picture-in-picture') {
        out.webkitSetPresentationMode('inline');
      }
      if (document.pictureInPictureElement && document.exitPictureInPicture) {
        document.exitPictureInPicture();
      }
    }
  } catch (e) {}

  clearInterval(window.__officeFloatWatch);
  window.__officeFloatWatch = null;
  document.documentElement.classList.remove('office-floating');
  var sheet = document.getElementById('office-float');
  if (sheet) sheet.textContent = '';
  var video = document.querySelector('[data-office-float]');
  if (video) video.removeAttribute('data-office-float');
  return 'landed';
})();
