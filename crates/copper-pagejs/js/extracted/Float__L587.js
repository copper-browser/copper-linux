// from Sources/Search/Float.swift:587 Isolate.toggle
(function () {
  var video = document.querySelector('[data-office-float]')
    || document.querySelector('video');
  if (!video) return true;
  if (video.paused) { video.play(); } else { video.pause(); }
  return !video.paused;
})();
