// from Sources/Search/Fork/Canvas/CanvasHost.swift:1062 CanvasHost.shareNow
// callAsyncJavaScript body on the Mac; the body is verbatim, its arguments are destructured.
async ({ s }) => {
const c = window.copperCanvas; if (c && typeof c.setShare === 'function') c.setShare(s);
}
