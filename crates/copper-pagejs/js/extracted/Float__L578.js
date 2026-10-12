// from Sources/Search/Float.swift:578 Isolate.where_
(function () {
  var video = document.querySelector('[data-office-float]')
    || document.querySelector('video');
  if (!video || !video.duration || !isFinite(video.duration)) return [0, true];
  return [video.currentTime / video.duration, !video.paused];
})();
