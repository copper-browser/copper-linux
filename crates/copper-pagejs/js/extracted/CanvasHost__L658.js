// from Sources/Search/Fork/Canvas/CanvasHost.swift:658 CanvasHost.ready
const c = window.copperCanvas;
if (!c || typeof c.init !== 'function') throw new Error('copperCanvas.init is missing');
await c.init(options);
for (const u of updates) c.applyUpdate(u);
if (typeof c.theme === 'function') c.theme(theme);
document.title = title;
window.__copperCanvasHost = ticket;
return true;
