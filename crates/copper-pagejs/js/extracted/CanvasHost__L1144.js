// from Sources/Search/Fork/Canvas/CanvasHost.swift:1144 CanvasHost.renamed
document.title = t;
const c = window.copperCanvas;
if (c && typeof c.rename === 'function') { await c.rename(t); return true; }
return false;
