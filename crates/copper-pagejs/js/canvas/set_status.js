// from Sources/Search/Fork/Canvas/CanvasHost.swift:1109 CanvasHost.statusNow
// callAsyncJavaScript body on the Mac; the body is verbatim, its arguments are destructured.
async ({ s }) => {
const c = window.copperCanvas;
if (c && typeof c.setStatus === 'function') c.setStatus(s);
}
