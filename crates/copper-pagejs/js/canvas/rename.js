// from Sources/Search/Fork/Canvas/CanvasHost.swift:1144 CanvasHost.renamed
// callAsyncJavaScript body on the Mac; the body is verbatim, its arguments are destructured.
async ({ t }) => {
document.title = t;
const c = window.copperCanvas;
if (c && typeof c.rename === 'function') { await c.rename(t); return true; }
return false;
}
