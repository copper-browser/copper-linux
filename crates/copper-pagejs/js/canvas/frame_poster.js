// from Sources/Search/Fork/Canvas/CanvasFrames.swift:1 CanvasHost.framePoster
// contract 2026-10-16 A.3 (the Mac call site's line lands with wave 2); the arguments are destructured.
async ({ name, url }) => {
const c = window.copperCanvas;
if (!c || typeof c.framePoster !== 'function') return false;
c.framePoster(name, url);
return true;
}
