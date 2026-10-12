// from Sources/Search/Fork/Canvas/CanvasHost.swift:658 CanvasHost.ready
// callAsyncJavaScript body on the Mac; the body is verbatim, its arguments are destructured.
async ({ options, updates, theme, title, ticket }) => {
const c = window.copperCanvas;
if (!c || typeof c.init !== 'function') throw new Error('copperCanvas.init is missing');
await c.init(options);
for (const u of updates) c.applyUpdate(u);
if (typeof c.theme === 'function') c.theme(theme);
document.title = title;
window.__copperCanvasHost = ticket;
return true;
}
