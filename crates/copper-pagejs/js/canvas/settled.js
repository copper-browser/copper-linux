// from Sources/Search/Fork/Canvas/CanvasHost.swift:1025 CanvasHost.picture
// Canvas-perf contract E: before the board's picture, wait (bounded) until what is on screen has
// decoded / loaded. Answers the page's {settled, waitedMs, pending: {images, frames}}, or null
// when the page predates settled() (older pages: snapshot at once). Never throws.
async ({ ms }) => {
const c = window.copperCanvas;
if (!c || typeof c.settled !== 'function') return null;
try {
  const out = await c.settled(ms);
  return out && typeof out === 'object' ? out : null;
} catch (e) {
  return null;
}
}
