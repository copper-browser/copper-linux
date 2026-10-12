// from Sources/Search/Forms.swift:90 FormRelay.withoutPasskeys
// user script: document start, all frames, page world (only while passkeys are not offered).
function () {
  try {
    Object.defineProperty(window, 'PublicKeyCredential', {
      value: undefined, configurable: true, writable: true
    });
  } catch (e) {
    try { delete window.PublicKeyCredential; } catch (ignored) {}
  }
}
