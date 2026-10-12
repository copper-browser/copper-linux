// from Sources/Search/Fork/Canvas/CanvasFrames.swift:106 CanvasHost.tellFrameHost
// callAsyncJavaScript body on the Mac; the body is verbatim, its arguments are destructured.
async ({ f }) => {
const c = window.copperCanvas; if (c && typeof c.frameHost === 'function') c.frameHost(f);
}
