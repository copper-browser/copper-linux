// from Sources/Search/Fork/Canvas/CanvasHost.swift:711 CanvasHost.watchAppearance
// callAsyncJavaScript body on the Mac; the body is verbatim, its arguments are destructured.
async ({ t }) => {
if (typeof window.copperCanvas.theme === 'function') window.copperCanvas.theme(t)
}
