// from Sources/Search/Float.swift:566 Isolate.skip
(function () {
  var video = document.querySelector('[data-office-float]')
    || document.querySelector('video');
  if (!video) return false;
  video.currentTime = Math.max(0, video.currentTime + (/*INTERP: seconds*/));
  return true;
})();
