// from Sources/Search/Forms.swift:90 FormRelay.withoutPasskeys
(function () {
  try {
    Object.defineProperty(window, 'PublicKeyCredential', {
      value: undefined, configurable: true, writable: true
    });
  } catch (e) {
    try { delete window.PublicKeyCredential; } catch (ignored) {}
  }
})();
