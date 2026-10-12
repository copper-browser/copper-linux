// from Sources/Search/Forms.swift:102 FormRelay.script
(function () {
  if (window.__officeForms) return;

  // The password box, and the last box before it that could hold a name.
  function pair() {
    var boxes = document.querySelectorAll('input[type="password"]');
    var pass = null;
    for (var p = 0; p < boxes.length; p++) {
      var b = boxes[p];
      var r = b.getBoundingClientRect();
      if (r.width > 0 && r.height > 0) { pass = b; break; }
    }
    if (!pass) return null;
    var scope = pass.form || (pass.closest && pass.closest('form')) || document;
    var all = scope.querySelectorAll('input');
    var user = null;
    for (var i = 0; i < all.length; i++) {
      if (all[i] === pass) break;
      var kind = (all[i].type || 'text').toLowerCase();
      if (kind === 'text' || kind === 'email' || kind === 'tel') user = all[i];
    }
    return { user: user, pass: pass };
  }

  function put(box, value) {
    if (!box) return;
    var tag = (box.tagName || '').toLowerCase();
    if (tag === 'select') {
      box.value = String(value);
      box.dispatchEvent(new Event('change', { bubbles: true }));
      return;
    }
    var prototype = tag === 'textarea'
      ? window.HTMLTextAreaElement.prototype
      : window.HTMLInputElement.prototype;
    var setter = Object.getOwnPropertyDescriptor(prototype, 'value');
    if (setter && setter.set) { setter.set.call(box, value); } else { box.value = value; }
    box.dispatchEvent(new Event('input', { bubbles: true }));
    box.dispatchEvent(new Event('change', { bubbles: true }));
    box.dispatchEvent(new Event('keyup', { bubbles: true }));
    box.dispatchEvent(new Event('blur', { bubbles: true }));
  }

  // What was typed by hand and not yet sent, box by box. A page whose
  // boxes still hold it is not put to sleep: waking it couldn't bring
  // that back. A box emptied by sending — a chat's composer — no longer
  // counts, and neither does a search box.
  var typed = [];
  document.addEventListener('input', function (e) {
    if (!e.isTrusted) return;
    var el = e.target;
    if (!el || typed.indexOf(el) >= 0) return;
    typed.push(el);
    if (typed.length > 40) typed.shift();
  }, true);
  function unsaved() {
    for (var i = 0; i < typed.length; i++) {
      var el = typed[i];
      if (!el.isConnected) continue;
      var tag = (el.tagName || '').toLowerCase();
      if (tag === 'textarea') {
        if (el.value.trim() && el.value !== el.defaultValue) return true;
      } else if (tag === 'input') {
        var kind = (el.type || 'text').toLowerCase();
        if (['text', 'email', 'url', 'tel', 'number'].indexOf(kind) < 0) continue;
        if (el.value.trim() && el.value !== el.defaultValue) return true;
      } else if (el.isContentEditable) {
        if ((el.textContent || '').trim()) return true;
      }
    }
    return false;
  }

  function visible(box) {
    if (!box || box.hidden) return false;
    var r = box.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) return false;
    var style = window.getComputedStyle(box);
    return style.display !== 'none' && style.visibility !== 'hidden' &&
      style.opacity !== '0';
  }

  function groupFor(kind) {
    if (['username', 'password', 'otp'].indexOf(kind) >= 0) return 'login';
    if (['cardNumber', 'cardName', 'cardExpMonth', 'cardExpYear', 'cardExp',
         'cardCode', 'cardBrand'].indexOf(kind) >= 0) return 'card';
    if (kind === 'other') return 'other';
    return 'identity';
  }

  function control(el) {
    if (!el || !visible(el)) return false;
    var tag = (el.tagName || '').toLowerCase();
    if (['input', 'select', 'textarea'].indexOf(tag) < 0) return false;
    if (tag !== 'input') return true;
    var type = (el.type || 'text').toLowerCase();
    return ['hidden', 'checkbox', 'radio', 'submit', 'button', 'reset',
            'image', 'file', 'range', 'color', 'search'].indexOf(type) < 0;
  }

  function labelParts(el) {
    var parts = [];
    function add(value) {
      value = (value || '').replace(/\s+/g, ' ').trim();
      if (value && parts.indexOf(value) < 0) parts.push(value);
    }
    var id = el.id || '';
    if (id) {
      var labels = document.getElementsByTagName('label');
      for (var i = 0; i < labels.length; i++) {
        if (labels[i].htmlFor === id) add(ownText(labels[i]));
      }
    }
    // A label wrapped around a select would otherwise read as every
    // option in it — "Country United States Canada" says "unit" too.
    var wrapping = el.closest && el.closest('label');
    if (wrapping) add(ownText(wrapping));
    var described = el.getAttribute('aria-labelledby') || '';
    described.split(/\s+/).forEach(function (name) {
      if (name) {
        var node = document.getElementById(name);
        if (node) add(node.innerText || node.textContent);
      }
    });
    add(el.getAttribute('aria-label'));
    add(el.getAttribute('placeholder'));
    add(el.name);
    add(id);
    return parts;
  }

  // A label's words without the words of the controls inside it.
  function ownText(node) {
    var copy = node.cloneNode(true);
    var inner = copy.querySelectorAll('input, select, textarea, option');
    for (var i = 0; i < inner.length; i++) {
      if (inner[i].parentNode) inner[i].parentNode.removeChild(inner[i]);
    }
    return copy.innerText || copy.textContent || '';
  }

  function fieldLabel(el) {
    var parts = labelParts(el);
    return parts.length ? parts[0] : '';
  }

  function autocompleteKind(el) {
    var value = (el.autocomplete || el.getAttribute('autocomplete') || '').toLowerCase();
    var tokens = value.split(/\s+/);
    var names = {
      'cc-number': 'cardNumber', 'cc-name': 'cardName', 'cc-exp': 'cardExp',
      'cc-exp-month': 'cardExpMonth', 'cc-exp-year': 'cardExpYear',
      'cc-csc': 'cardCode', 'cc-type': 'cardBrand',
      'given-name': 'firstName', 'additional-name': 'middleName',
      'family-name': 'lastName', 'name': 'fullName', 'email': 'email',
      'tel': 'phone', 'tel-national': 'phone', 'organization': 'company',
      'street-address': 'address1', 'address-line1': 'address1',
      'address-line2': 'address2', 'address-line3': 'address3',
      'address-level1': 'state', 'address-level2': 'city',
      'postal-code': 'postalCode', 'country': 'country',
      'country-name': 'country', 'username': 'username',
      'current-password': 'password', 'new-password': 'password',
      'one-time-code': 'otp'
    };
    for (var i = tokens.length - 1; i >= 0; i--) {
      if (names[tokens[i]]) return names[tokens[i]];
    }
    return null;
  }

  function regexKind(el) {
    var clues = labelParts(el).join(' ').toLowerCase();
    if (/user.?name|user.?id|login/.test(clues)) return 'username';
    if (/otp|one.?time|totp|2fa|mfa|verification.?code/.test(clues)) return 'otp';
    if (/card.?holder|card.?name|name.?on.?card/.test(clues)) return 'cardName';
    if (/exp(?:iration|iry)?[ ._-]*month|exp.?month|month.*exp/.test(clues)) return 'cardExpMonth';
    if (/exp(?:iration|iry)?[ ._-]*(?:year)|exp.?year|year.*exp/.test(clues)) return 'cardExpYear';
    if (/cc.?type|card.?brand|brand/.test(clues)) return 'cardBrand';
    if (/cc.?exp|expir|expdate|(^|[^a-z])exp([^a-z]|$)/.test(clues)) return 'cardExp';
    if (/cvv|cvc|csc|security[ ._-]*code/.test(clues)) return 'cardCode';
    if (/card.?number|ccnum|\bpan\b/.test(clues)) return 'cardNumber';
    if (/first.?name|fname|given/.test(clues)) return 'firstName';
    if (/middle.?name|mname/.test(clues)) return 'middleName';
    if (/last.?name|lname|surname|family/.test(clues)) return 'lastName';
    if (/full.?name|^name$/.test(clues)) return 'fullName';
    if (/street|address[ ._-]*(?:line[ ._-]*)?1|addr1/.test(clues)) return 'address1';
    if (/\bapt\b|\bsuite\b|\bunit\b|address[ ._-]*(?:line[ ._-]*)?2|addr2/.test(clues)) return 'address2';
    if (/address[ ._-]*(?:line[ ._-]*)?3|addr3/.test(clues)) return 'address3';
    if (/\bcity\b|\btown\b|locality/.test(clues)) return 'city';
    if (/\bstate\b|province|\bregion\b/.test(clues)) return 'state';
    if (/country/.test(clues)) return 'country';
    if (/zip|postal/.test(clues)) return 'postalCode';
    if (/phone|\btel\b|mobile/.test(clues)) return 'phone';
    if (/company|organi[sz]ation/.test(clues)) return 'company';
    if (/ssn|social/.test(clues)) return 'ssn';
    if (/passport/.test(clues)) return 'passportNumber';
    if (/licen[cs]e/.test(clues)) return 'licenseNumber';
    return null;
  }

  function selectKind(kind) {
    return ['state', 'country', 'cardExpMonth', 'cardExpYear', 'cardBrand']
      .indexOf(kind) >= 0;
  }

  function classifyCore(el) {
    if (!control(el)) return null;
    var tag = (el.tagName || '').toLowerCase();
    var type = (el.type || 'text').toLowerCase();
    var both = pair();
    var kind = null;
    // A password box is always a password, and the first box of a login
    // pair is its username even when its type says email or tel.
    if (tag === 'input' && type === 'password') kind = 'password';
    else if (both && both.user === el) kind = 'username';
    else kind = autocompleteKind(el);
    if (!kind) {
      if (tag === 'input' && type === 'email') kind = 'email';
      else if (tag === 'input' && type === 'tel') kind = 'phone';
      else kind = regexKind(el);
    }
    if (!kind || (tag === 'select' && !selectKind(kind))) return null;
    return { kind: kind, group: groupFor(kind), label: fieldLabel(el) };
  }

  function controlsIn(scope) {
    if (!scope || !scope.querySelectorAll) return [];
    return scope.querySelectorAll('input, select, textarea');
  }

  function hasClassified(scope, except) {
    var controls = controlsIn(scope);
    for (var i = 0; i < controls.length; i++) {
      if (controls[i] !== except && classifyCore(controls[i])) return true;
    }
    return false;
  }

  function classify(el) {
    var result = classifyCore(el);
    if (result) return result;
    var tag = el && (el.tagName || '').toLowerCase();
    var type = el && (el.type || 'text').toLowerCase();
    // Keep unlabelled custom text boxes available to the field picker when
    // they live beside at least one recognised field in the same form.
    if (control(el) && tag === 'input' && type === 'text' &&
        el.form && hasClassified(el.form, el)) {
      return { kind: 'other', group: 'other', label: fieldLabel(el) };
    }
    return null;
  }

  // The one-time-code box sites call by several names. Prefer the
  // autocomplete promise, then the clues in the field's own labels.
  function otpField() {
    var inputs = document.querySelectorAll('input');
    var clues = /otp|one.?time|verif|code|totp|2fa|mfa|token/i;
    var fallback = null;
    for (var i = 0; i < inputs.length; i++) {
      var input = inputs[i];
      if (!visible(input)) continue;
      var autocomplete = (input.autocomplete || input.getAttribute('autocomplete') || '').toLowerCase();
      if (autocomplete === 'one-time-code') return input;
      var kind = (input.type || 'text').toLowerCase();
      var inputmode = (input.inputMode || input.getAttribute('inputmode') || '').toLowerCase();
      var eligible = ['text', 'tel', 'number'].indexOf(kind) >= 0 ||
        inputmode === 'numeric' || inputmode === 'tel';
      var labels = [
        input.name,
        input.id,
        input.getAttribute('aria-label'),
        input.getAttribute('placeholder'),
        autocomplete
      ].join(' ');
      if (!fallback && eligible && clues.test(labels)) fallback = input;
    }
    return fallback;
  }

  // Some sign-in pages use one box for each digit. Find the nearest
  // ancestor that contains a complete, visible run of those boxes.
  function splitOTPFields() {
    var inputs = document.querySelectorAll('input');
    var singles = [];
    for (var i = 0; i < inputs.length; i++) {
      var input = inputs[i];
      if (!visible(input)) continue;
      if (input.maxLength !== 1 && input.getAttribute('maxlength') !== '1') continue;
      singles.push(input);
    }
    for (var first = 0; first < singles.length; first++) {
      var container = singles[first].parentElement;
      while (container && container !== document.documentElement) {
        var inside = [];
        for (var j = 0; j < singles.length; j++) {
          if (container.contains(singles[j])) inside.push(singles[j]);
        }
        if (inside.length >= 4 && inside.length <= 8) return inside;
        container = container.parentElement;
      }
    }
    return null;
  }

  var stateNames = {
    AL: 'Alabama', AK: 'Alaska', AZ: 'Arizona', AR: 'Arkansas', CA: 'California',
    CO: 'Colorado', CT: 'Connecticut', DE: 'Delaware', FL: 'Florida', GA: 'Georgia',
    HI: 'Hawaii', ID: 'Idaho', IL: 'Illinois', IN: 'Indiana', IA: 'Iowa', KS: 'Kansas',
    KY: 'Kentucky', LA: 'Louisiana', ME: 'Maine', MD: 'Maryland', MA: 'Massachusetts',
    MI: 'Michigan', MN: 'Minnesota', MS: 'Mississippi', MO: 'Missouri', MT: 'Montana',
    NE: 'Nebraska', NV: 'Nevada', NH: 'New Hampshire', NJ: 'New Jersey', NM: 'New Mexico',
    NY: 'New York', NC: 'North Carolina', ND: 'North Dakota', OH: 'Ohio', OK: 'Oklahoma',
    OR: 'Oregon', PA: 'Pennsylvania', RI: 'Rhode Island', SC: 'South Carolina',
    SD: 'South Dakota', TN: 'Tennessee', TX: 'Texas', UT: 'Utah', VT: 'Vermont',
    VA: 'Virginia', WA: 'Washington', WV: 'West Virginia', WI: 'Wisconsin',
    WY: 'Wyoming', DC: 'District of Columbia'
  };
  var countryNames = {
    US: 'United States', GB: 'United Kingdom', UK: 'United Kingdom', CA: 'Canada',
    DE: 'Germany', FR: 'France', ES: 'Spain', IT: 'Italy', NL: 'Netherlands',
    AU: 'Australia', MX: 'Mexico', BR: 'Brazil', JP: 'Japan', IN: 'India', CH: 'Switzerland',
    SE: 'Sweden', NO: 'Norway', DK: 'Denmark', IE: 'Ireland', PT: 'Portugal', BE: 'Belgium',
    AT: 'Austria'
  };

  function sameValue(a, b) {
    return String(a || '').trim().toLowerCase() === String(b || '').trim().toLowerCase();
  }

  function selectValues(kind, wanted) {
    var value = String(wanted == null ? '' : wanted).trim();
    var values = [value];
    function add(value) {
      if (values.indexOf(value) < 0) values.push(value);
    }
    if (kind === 'state') {
      var state = stateNames[value.toUpperCase()];
      if (state) add(state);
      for (var stateCode in stateNames) {
        if (stateNames.hasOwnProperty(stateCode) && sameValue(stateNames[stateCode], value)) add(stateCode);
      }
    }
    if (kind === 'country') {
      var country = countryNames[value.toUpperCase()];
      if (country) add(country);
      for (var countryCode in countryNames) {
        if (countryNames.hasOwnProperty(countryCode) && sameValue(countryNames[countryCode], value)) add(countryCode);
      }
    }
    if (kind === 'cardExpMonth') {
      var month = parseInt(value, 10);
      var months = {
        1: 'January', 2: 'February', 3: 'March', 4: 'April', 5: 'May', 6: 'June',
        7: 'July', 8: 'August', 9: 'September', 10: 'October', 11: 'November', 12: 'December'
      };
      if (!(month >= 1 && month <= 12)) {
        for (var monthNumber in months) {
          if (months.hasOwnProperty(monthNumber) &&
              (sameValue(months[monthNumber], value) ||
               sameValue(months[monthNumber].slice(0, 3), value))) {
            month = parseInt(monthNumber, 10);
            break;
          }
        }
      }
      if (month >= 1 && month <= 12) {
        add(String(month));
        add(month < 10 ? '0' + month : String(month));
        add(months[month]);
        add(months[month].slice(0, 3));
      }
    }
    if (kind === 'cardExpYear') {
      var year = parseInt(value, 10);
      if (year >= 0) {
        if (year < 100) add(String(2000 + year));
        else add(String(year).slice(-2));
      }
    }
    return values;
  }

  function selectChoice(box, kind, wanted) {
    var choices = selectValues(kind, wanted);
    var options = box.options || [];
    for (var i = 0; i < options.length; i++) {
      for (var j = 0; j < choices.length; j++) {
        if (sameValue(options[i].value, choices[j]) || sameValue(options[i].text, choices[j])) {
          return options[i].value;
        }
      }
    }
    return null;
  }

  function expiryParts(value) {
    var text = String(value == null ? '' : value).trim();
    var pieces = text.split(/[\/\-\s]+/);
    if (pieces.length >= 2) return { month: pieces[0], year: pieces[1] };
    var digits = text.replace(/[^0-9]/g, '');
    if (digits.length >= 6) return { month: digits.slice(0, 2), year: digits.slice(2) };
    if (digits.length >= 4) return { month: digits.slice(0, 2), year: digits.slice(2) };
    return { month: text, year: '' };
  }

  function expiryValue(value, box) {
    var parts = expiryParts(value);
    var placeholder = box.getAttribute('placeholder') || '';
    var four = (box.maxLength >= 7) || /Y{4}/i.test(placeholder);
    var year = String(parts.year || '');
    if (four && year.length === 2) year = '20' + year;
    if (!four && year.length > 2) year = year.slice(-2);
    var month = String(parts.month || '');
    if (month.length === 1) month = '0' + month;
    var separator = /\s\/|\/\s/.test(placeholder) ? ' / ' : '/';
    return month + separator + year;
  }

  function fillScope() {
    var active = document.activeElement;
    if (active && active.form) return active.form;
    var ancestor = active && active.parentElement;
    for (var level = 0; ancestor && level < 4; level++, ancestor = ancestor.parentElement) {
      var controls = controlsIn(ancestor);
      var found = 0;
      for (var i = 0; i < controls.length; i++) {
        if (classify(controls[i])) found++;
      }
      if (found >= 2) return ancestor;
    }
    return document;
  }

  function controlsForScope(scope) {
    return controlsIn(scope || document);
  }

  function fillValues(values) {
    if (!values) return 0;
    var scope = fillScope();
    var controls = controlsForScope(scope);
    var infos = [];
    var hasNameParts = false;
    for (var i = 0; i < controls.length; i++) {
      var info = classify(controls[i]);
      infos.push(info);
      if (info && (info.kind === 'firstName' || info.kind === 'lastName')) hasNameParts = true;
    }
    var active = document.activeElement;
    var filled = 0;
    for (var j = 0; j < controls.length; j++) {
      var box = controls[j];
      var info = infos[j];
      if (!info || (info.kind === 'fullName' && hasNameParts)) continue;
      var wanted = values[info.kind];
      if (wanted == null && info.kind === 'cardExpMonth' && values.cardExp != null) {
        wanted = expiryParts(values.cardExp).month;
      }
      if (wanted == null && info.kind === 'cardExpYear' && values.cardExp != null) {
        wanted = expiryParts(values.cardExp).year;
      }
      if (wanted == null) continue;
      var tag = (box.tagName || '').toLowerCase();
      var password = tag === 'input' && (box.type || '').toLowerCase() === 'password';
      if (box.value && box !== active && !password) continue;
      if (info.kind === 'cardExp') wanted = expiryValue(wanted, box);
      if (info.kind === 'cardExpYear' && tag !== 'select') {
        var yearText = String(wanted);
        var placeholder = box.getAttribute('placeholder') || '';
        var twoDigit = box.maxLength === 2 || /Y{2}(?!Y)/i.test(placeholder);
        wanted = twoDigit ? yearText.slice(-2) :
          (yearText.length === 2 ? '20' + yearText : yearText);
      }
      if (tag === 'select') {
        var choice = selectChoice(box, info.kind, wanted);
        if (choice == null) continue;
        put(box, choice);
      } else {
        put(box, wanted);
      }
      filled++;
    }
    return filled;
  }

  function fillFocused(value) {
    var active = document.activeElement;
    if (!active || ['input', 'select', 'textarea'].indexOf((active.tagName || '').toLowerCase()) < 0) return false;
    put(active, value);
    return true;
  }

  function normalised(value) {
    return String(value || '').toLowerCase().replace(/[\s\-_:*]/g, '');
  }

  function fillField(label, value) {
    var wanted = normalised(label);
    if (!wanted) return false;
    var controls = document.querySelectorAll('input, select, textarea');
    for (var i = 0; i < controls.length; i++) {
      var info = classify(controls[i]);
      if (!info) continue;
      var have = normalised(info.label);
      if (have && (have.indexOf(wanted) >= 0 || wanted.indexOf(have) >= 0)) {
        var tag = (controls[i].tagName || '').toLowerCase();
        if (tag === 'select') {
          var choice = selectChoice(controls[i], info.kind, value);
          if (choice == null) continue;
          put(controls[i], choice);
        } else {
          put(controls[i], info.kind === 'cardExp' ? expiryValue(value, controls[i]) : value);
        }
        return true;
      }
    }
    return false;
  }

  function hasFields(group) {
    var controls = document.querySelectorAll('input, select, textarea');
    for (var i = 0; i < controls.length; i++) {
      var info = classify(controls[i]);
      if (info && info.group === group) return true;
    }
    return false;
  }

  function fieldsPresent() {
    var controls = document.querySelectorAll('input, select, textarea');
    var seen = {};
    var result = [];
    for (var i = 0; i < controls.length; i++) {
      var info = classify(controls[i]);
      if (info && !seen[info.kind]) {
        seen[info.kind] = true;
        result.push(info.kind);
      }
    }
    return result;
  }

  window.__officeForms = {
    unsaved: unsaved,
    fill: function (user, password) {
      var both = pair();
      if (!both) return false;
      if (both.user && !both.user.value) put(both.user, user);
      put(both.pass, password);
      return true;
    },
    submit: function () {
      var both = pair();
      if (!both) return false;
      ['keydown', 'keypress', 'keyup'].forEach(function (kind) {
        var event = new KeyboardEvent(kind, {
          key: 'Enter', code: 'Enter', keyCode: 13, which: 13,
          bubbles: true, cancelable: true
        });
        try { Object.defineProperty(event, 'keyCode', { value: 13 }); } catch (ignored) {}
        try { Object.defineProperty(event, 'which', { value: 13 }); } catch (ignored) {}
        both.pass.dispatchEvent(event);
      });
      if (both.pass.form) {
        if (typeof both.pass.form.requestSubmit === 'function') {
          both.pass.form.requestSubmit();
        } else if (typeof both.pass.form.submit === 'function') {
          both.pass.form.submit();
        } else {
          return false;
        }
        return true;
      }
      var container = both.pass.parentElement;
      while (container) {
        var button = container.querySelector(
          'button[type="submit"], input[type="submit"], button'
        );
        if (button) {
          button.click();
          return true;
        }
        container = container.parentElement;
      }
      return false;
    },
    otpField: otpField,
    fillOTP: function (code) {
      var split = splitOTPFields();
      if (split) {
        if (String(code).length < split.length) return false;
        for (var i = 0; i < split.length; i++) put(split[i], String(code).charAt(i));
        return true;
      }
      var box = otpField();
      if (!box) return false;
      put(box, code);
      return true;
    },
    hasOTP: function () { return !!otpField(); },
    // Whether there is still a sign-in on the page. Asked after a
    // password went out, to tell a sign-in that took from one refused.
    hasPassword: function () { return !!pair(); },
    classify: classify,
    fillValues: fillValues,
    fillFocused: fillFocused,
    fillField: fillField,
    hasFields: hasFields,
    fieldsPresent: fieldsPresent
  };

  // What is in the boxes when they are sent. Said every time — a click
  // on "show password" says it too — because the browser only listens
  // once the page has moved on, and keeps the last thing it heard.
  function offer() {
    var both = pair();
    if (!both || !both.pass.value) return;
    window.webkit.messageHandlers.officeForms.postMessage({
      kind: 'submit',
      user: both.user ? both.user.value : '',
      password: both.pass.value
    });
  }

  document.addEventListener('submit', offer, true);
  document.addEventListener('keydown', function (e) {
    if (e.key !== 'Enter') return;
    var both = pair();
    if (both && (document.activeElement === both.pass || document.activeElement === both.user)) offer();
  }, true);
  // Plenty of sign-in buttons aren't in a form and never fire submit.
  document.addEventListener('click', function (e) {
    var el = e.target;
    if (!el || !el.closest) return;
    if (el.closest('button, input[type="submit"], [role="button"]')) {
      setTimeout(offer, 0);
    }
  }, true);

  var told = false;
  function tell() {
    if (told || !pair()) return;
    told = true;
    window.webkit.messageHandlers.officeForms.postMessage({ kind: 'form' });
  }
  if (document.readyState === 'complete') { tell(); }
  else { window.addEventListener('load', tell); }
  // A form the page builds for itself, a moment after it loads — or the
  // password step of a sign-in that asks for the name first.
  setTimeout(tell, 700);
  setTimeout(tell, 2200);
  // The boxes going away without a new page — a sign-in done in place —
  // is the other way a sign-in shows it took.
  var settling = null;
  new MutationObserver(function () {
    if (!told) { tell(); return; }
    if (pair()) return;
    told = false;
    clearTimeout(settling);
    settling = setTimeout(function () {
      if (pair()) return;
      window.webkit.messageHandlers.officeForms.postMessage({ kind: 'settled' });
    }, 400);
  }).observe(document.documentElement, { childList: true, subtree: true });

  // Whether the caret is somewhere on the page that takes typing.
  //
  // The browser gives Tab to its own row of tabs, which is right until you
  // are filling something in: plenty of fields offer a completion you take
  // with Tab, and stealing the key there would make them unusable.
  function editable(el) {
    if (!el) return false;
    var tag = (el.tagName || '').toLowerCase();
    if (tag === 'textarea') return true;
    if (el.isContentEditable === true) return true;
    if (tag !== 'input') return false;
    var kind = (el.type || 'text').toLowerCase();
    return ['text', 'search', 'email', 'url', 'tel', 'password', 'number',
            'date', 'datetime-local', 'month', 'week', 'time'].indexOf(kind) >= 0;
  }

  // Which account the page is already talking about: what is typed in
  // the name box, or — on the second step of a sign-in, where the name
  // is printed above the password box — an address in the text nearby.
  // Copper puts that account first in its list.
  function hint(both) {
    if (!both) return '';
    if (both.user && both.user.value) return both.user.value.trim();
    var scope = both.pass.form || (both.pass.closest && both.pass.closest('form, main, [role=main], section, div')) || document.body;
    for (var i = 0; i < 4 && scope; i++) {
      var text = (scope.innerText || '');
      var m = text.match(/[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+[.][A-Za-z]{2,}/);
      if (m) return m[0];
      if (scope === document.body) break;
      scope = scope.parentElement;
    }
    return '';
  }

  function caret() {
    var el = document.activeElement;
    var both = pair();
    var field = classify(el);
    var rect = null;
    if (field && visible(el)) {
      var r = el.getBoundingClientRect();
      if (r.width > 0 && r.height > 0) rect = { x: r.left, y: r.top, w: r.width, h: r.height };
      else field = null;
    }
    var login = both && el && (el === both.user || el === both.pass);
    window.webkit.messageHandlers.officeForms.postMessage({
      kind: 'focus',
      typing: editable(el),
      rect: rect,
      field: field,
      hint: rect && login ? hint(both) : ''
    });
  }

  // The box moves when the page scrolls or the window changes size, and
  // whatever hangs from it has to move too. Once a frame at most.
  var moving = false;
  function moved() {
    if (moving) return;
    moving = true;
    requestAnimationFrame(function () { moving = false; caret(); });
  }
  window.addEventListener('scroll', moved, true);
  window.addEventListener('resize', moved);

  // Going full screen, announced before it happens rather than after.
  //
  // WebKit puts the video in a window of its own and slides ours away
  // behind it. For a frame or two ours is still on screen, and everything
  // this browser draws is white — which is the pale band across the top of
  // the animation. Knowing a moment early is enough to paint it black.
  function immersed() {
    var on = !!(document.fullscreenElement || document.webkitFullscreenElement);
    window.webkit.messageHandlers.officeForms.postMessage({
      kind: 'fullscreen', on: on
    });
  }
  document.addEventListener('fullscreenchange', immersed, true);
  document.addEventListener('webkitfullscreenchange', immersed, true);

  // The asking, caught before the animation starts.
  ['requestFullscreen', 'webkitRequestFullscreen', 'webkitRequestFullScreen']
    .forEach(function (name) {
      var was = Element.prototype[name];
      if (!was) return;
      Element.prototype[name] = function () {
        window.webkit.messageHandlers.officeForms.postMessage({
          kind: 'fullscreen', on: true
        });
        return was.apply(this, arguments);
      };
    });

  document.addEventListener('focusin', caret, true);
  document.addEventListener('focusout', function () { setTimeout(caret, 0); }, true);
  document.addEventListener('mouseup', function () { setTimeout(caret, 0); }, true);
  caret();
})();
