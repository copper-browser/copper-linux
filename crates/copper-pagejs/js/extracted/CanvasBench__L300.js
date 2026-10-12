// from Sources/Search/Fork/Canvas/CanvasBench.swift:300 CanvasBench.perfStop
(function () {
  var P = window.__copperPerf;
  if (!P) return JSON.stringify({ error: 'canvas perf start first' });
  cancelAnimationFrame(P.raf);
  window.removeEventListener('wheel', P.onWheel, true);
  window.__copperPerf = null;
  function pick(s, p) { return s.length ? s[Math.min(s.length - 1, Math.floor(p / 100 * s.length))] : 0; }
  function r(n) { return Math.round(n * 10) / 10; }
  var s = P.deltas.slice().sort(function (a, b) { return a - b; });
  var l = P.lag.slice().sort(function (a, b) { return a - b; });
  return JSON.stringify({
    frames: s.length, p50: r(pick(s, 50)), p95: r(pick(s, 95)), max: r(s[s.length - 1] || 0),
    over16: P.deltas.filter(function (d) { return d > 17.5; }).length,
    over33: P.deltas.filter(function (d) { return d > 34; }).length,
    ms: Math.round(performance.now() - P.began),
    // The event's clock and the page's differ by a constant, so lag is
    // read above the gesture's quickest event: how much later than its
    // best the board took each one.
    wheel: l.length, lagP50: r(pick(l, 50) - (l[0] || 0)), lagP95: r(pick(l, 95) - (l[0] || 0)),
    lagMax: r((l[l.length - 1] || 0) - (l[0] || 0))
  });
})()
