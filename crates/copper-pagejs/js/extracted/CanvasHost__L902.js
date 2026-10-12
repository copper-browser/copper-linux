// from Sources/Search/Fork/Canvas/CanvasHost.swift:902 CanvasHost.checkpoint
const c = window.copperCanvas;
if (!c || typeof c.exportState !== 'function') return null;
return await c.exportState();
