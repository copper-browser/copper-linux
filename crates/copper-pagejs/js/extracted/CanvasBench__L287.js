// from Sources/Search/Fork/Canvas/CanvasBench.swift:287 CanvasBench.perfStart
(function () {
  var P = window.__copperPerf;
  if (P) { cancelAnimationFrame(P.raf); window.removeEventListener('wheel', P.onWheel, true); }
  P = window.__copperPerf = { deltas: [], last: null, raf: 0, began: performance.now(), lag: [] };
  function loop(t) { if (P.last !== null) P.deltas.push(t - P.last); P.last = t; P.raf = requestAnimationFrame(loop); }
  P.raf = requestAnimationFrame(loop);
  P.onWheel = function (e) { P.lag.push(performance.now() - e.timeStamp); };
  window.addEventListener('wheel', P.onWheel, { passive: true, capture: true });
  return JSON.stringify({ started: true });
})()
