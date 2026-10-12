// from Sources/Search/Fork/Passkeys/Passkeys.swift:22 Passkeys.script
(function () {
  if (window.__copperPasskeys) return;
  window.__copperPasskeys = true;

  var originalCreate = navigator.credentials && navigator.credentials.create;
  var originalGet = navigator.credentials && navigator.credentials.get;
  var te = new TextEncoder();
  var td = new TextDecoder();

  function b64(bytes) {
    var a = new Uint8Array(bytes), s = '';
    for (var i = 0; i < a.length; i += 0x8000) {
      s += String.fromCharCode.apply(null, a.subarray(i, Math.min(i + 0x8000, a.length)));
    }
    return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '');
  }
  function bytes(text) {
    var s = String(text || '').replace(/-/g, '+').replace(/_/g, '/');
    while (s.length % 4) s += '=';
    var raw = atob(s), out = new Uint8Array(raw.length);
    for (var i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
    return out.buffer;
  }
  function isBytes(value) {
    return value instanceof ArrayBuffer || (value && ArrayBuffer.isView(value));
  }
  function jsonValue(value) {
    if (isBytes(value)) return b64(value.buffer || value);
    if (Array.isArray(value)) return value.map(jsonValue);
    if (!value || typeof value !== 'object') return value;
    var out = {};
    Object.keys(value).forEach(function (key) { out[key] = jsonValue(value[key]); });
    return out;
  }
  function jsonOptions(options, kind) {
    var p = options && options.publicKey;
    if (!p) return null;
    var out = jsonValue(p);
    if (p.user) out.user = jsonValue(p.user);
    out.mediation = options.mediation || undefined;
    out.kind = kind;
    return out;
  }
  function domError(name, message) {
    try { return new DOMException(message || name, name); }
    catch (_) { var e = new Error(message || name); e.name = name; return e; }
  }
  function fail(result) {
    var name = result && result.error ? result.error : 'NotAllowedError';
    throw domError(name, result && result.message);
  }
  function publicCredential(id, response, extensions) {
    var item = Object.create(PublicKeyCredential.prototype);
    Object.defineProperties(item, {
      id: { value: id, enumerable: true },
      rawId: { value: bytes(id), enumerable: true },
      type: { value: 'public-key', enumerable: true },
      response: { value: response, enumerable: true },
      authenticatorAttachment: { value: 'platform', enumerable: true }
    });
    item.getClientExtensionResults = function () { return extensions || {}; };
    item.toJSON = function () {
      return { id: item.id, rawId: b64(item.rawId), type: item.type,
               response: response && response.toJSON ? response.toJSON() : {},
               authenticatorAttachment: item.authenticatorAttachment };
    };
    return item;
  }
  function attestation(result, requested) {
    var response = {
      clientDataJSON: bytes(result.clientDataJSON),
      attestationObject: bytes(result.attestationObject),
      getTransports: function () { return ['internal', 'hybrid']; },
      getAuthenticatorData: function () { return bytes(result.authenticatorData); },
      getPublicKey: function () { return result.publicKey ? bytes(result.publicKey) : null; },
      getPublicKeyAlgorithm: function () { return -7; },
      toJSON: function () {
        return { clientDataJSON: b64(this.clientDataJSON), attestationObject: b64(this.attestationObject) };
      }
    };
    var extensions = requested && requested.extensions && requested.extensions.credProps
      ? { credProps: { rk: true } } : {};
    return publicCredential(result.id, response, extensions);
  }
  function assertion(result) {
    var response = {
      clientDataJSON: bytes(result.clientDataJSON),
      authenticatorData: bytes(result.authenticatorData),
      signature: bytes(result.signature),
      userHandle: result.userHandle ? bytes(result.userHandle) : null,
      toJSON: function () {
        return { clientDataJSON: b64(this.clientDataJSON), authenticatorData: b64(this.authenticatorData),
                 signature: b64(this.signature), userHandle: this.userHandle ? b64(this.userHandle) : null };
      }
    };
    return publicCredential(result.id, response, result.extensions || {});
  }
  function call(kind, options) {
    var p = options && options.publicKey;
    if (!p) return null;
    if (options.signal && options.signal.aborted) return Promise.reject(domError('AbortError', 'The operation was aborted'));
    if (options.mediation === 'conditional') return Promise.reject(domError('NotSupportedError', 'Conditional mediation is not available'));
    var payload = jsonOptions(options, kind);
    payload.origin = location.origin;
    try { payload.topOrigin = top.location.origin; } catch (_) { payload.topOrigin = location.origin; }
    payload.crossOrigin = window !== top;
    payload.rpId = p.rp || location.hostname;
    payload.mediation = options.mediation || '';
    var message = window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.copperPasskeys;
    if (!message) return Promise.reject(domError('NotSupportedError', 'Copper passkeys are unavailable'));
    payload.requestId = Math.random().toString(36).slice(2);
    var reply = Promise.resolve(message.postMessage(payload)).then(function (result) {
      if (!result || result.error) return fail(result);
      return kind === 'create' ? attestation(result, p) : assertion(result);
    });
    if (options.signal) {
      reply = Promise.race([reply, new Promise(function (_, reject) {
        options.signal.addEventListener('abort', function () {
          reject(domError('AbortError', 'The operation was aborted'));
        }, { once: true });
      })]);
    }
    return reply;
  }

  function PublicKeyCredential() {}
  PublicKeyCredential.prototype = {};
  Object.defineProperty(PublicKeyCredential.prototype, 'constructor', { value: PublicKeyCredential });
  PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable = function () { return Promise.resolve(true); };
  PublicKeyCredential.isConditionalMediationAvailable = function () { return Promise.resolve(false); };
  PublicKeyCredential.getClientCapabilities = function () { return Promise.resolve({
    userVerifyingPlatformAuthenticator: true, conditionalGet: false, hybridTransport: false,
    passkeyPlatformAuthenticator: true, relatedOrigins: false, signalAllAcceptedCredentials: false,
    userVerifyingPlatformAuthenticatorAvailable: true
  }); };
  PublicKeyCredential.parseCreationOptionsFromJSON = function (value) {
    var out = Object.assign({}, value || {});
    if (out.challenge) out.challenge = bytes(out.challenge);
    if (out.user) out.user = Object.assign({}, out.user, { id: bytes(out.user.id) });
    if (out.excludeCredentials) out.excludeCredentials = out.excludeCredentials.map(function (c) {
      return Object.assign({}, c, { id: bytes(c.id) });
    });
    return out;
  };
  PublicKeyCredential.parseRequestOptionsFromJSON = function (value) {
    var out = Object.assign({}, value || {});
    if (out.challenge) out.challenge = bytes(out.challenge);
    if (out.allowCredentials) out.allowCredentials = out.allowCredentials.map(function (c) {
      return Object.assign({}, c, { id: bytes(c.id) });
    });
    return out;
  };
  window.PublicKeyCredential = PublicKeyCredential;

  if (navigator.credentials) {
    var create = function (options) {
      var result = call('create', options);
      return result || originalCreate.call(navigator.credentials, options);
    };
    var get = function (options) {
      var result = call('get', options);
      return result || originalGet.call(navigator.credentials, options);
    };
    function installCredentials() {
      var credentials = navigator.credentials;
      var prototype = Object.getPrototypeOf(credentials);
      try {
        Object.defineProperty(credentials, 'create', { value: create, configurable: true, writable: true });
        Object.defineProperty(credentials, 'get', { value: get, configurable: true, writable: true });
      } catch (_) {}
      try {
        Object.defineProperty(prototype, 'create', { value: create, configurable: true, writable: true });
        Object.defineProperty(prototype, 'get', { value: get, configurable: true, writable: true });
      } catch (_) {}
    }
    installCredentials();
    if (window.queueMicrotask) queueMicrotask(installCredentials);
    setTimeout(installCredentials, 0);
  }
})();
