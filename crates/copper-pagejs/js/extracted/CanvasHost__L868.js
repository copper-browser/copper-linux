// from Sources/Search/Fork/Canvas/CanvasHost.swift:868 CanvasHost.compactIfNeeded
const c = window.copperCanvas;
if (typeof c.exportState !== 'function') return null;
return await c.exportState();
