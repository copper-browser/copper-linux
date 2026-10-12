// from Sources/Search/Fork/MCP/Probe.swift:22 Probe.script
// callAsyncJavaScript body on the Mac with arguments probeSeconds/probeTop/probeFormat; the body is verbatim.
async ({ probeSeconds, probeTop, probeFormat }) => {
return (async function () {
  var duration = Math.max(1, Math.min(10, Number(probeSeconds) || 3));
  var rowLimit = Math.max(1, Math.min(100, Math.floor(Number(probeTop) || 8)));
  var outputFormat = probeFormat === 'json' ? 'json' : 'text';
  var nativeRAF = window.requestAnimationFrame;
  var nativeTimeout = window.setTimeout;
  var nativeInterval = window.setInterval;
  var rafScheduled = 0;
  var rafCallbacks = 0;
  var timeoutScheduled = 0;
  var intervalScheduled = 0;
  var rafStacks = Object.create(null);
  var mutations = 0;
  var mutationGroups = Object.create(null);
  var longTaskCount = 0;
  var longTaskMs = 0;
  var resources = [];
  var mutationObserver = null;
  var longTaskObserver = null;
  var resourceObserver = null;
  var started = performance.now();

  function trimFrame(frame) {
    return String(frame || '').replace(/^\s+/, '').replace(/^at\s+/, '').slice(0, 240);
  }

  // The first two useful frames after the Error/wrapper frames are a
  // stable-enough call-site key for the short, deliberately rough loop count.
  function callSite() {
    try {
      var lines = String((new Error()).stack || '').split('\n');
      return lines.slice(2, 4).map(trimFrame).filter(Boolean).join(' ← ') || '(unknown call site)';
    } catch (e) {
      return '(unknown call site)';
    }
  }

  function count(map, key, amount) {
    map[key] = (map[key] || 0) + (amount == null ? 1 : amount);
  }

  function classSuffix(element) {
    if (!element || element.nodeType !== 1) return '';
    var value = element.getAttribute('class') || '';
    return value.trim().split(/\s+/).filter(Boolean).map(function (part) {
      return String(part).replace(/[^A-Za-z0-9_-]/g, '_');
    }).join('.');
  }

  function elementLabel(element) {
    if (!element || element.nodeType !== 1) return '(document)';
    var tag = String(element.tagName || 'element').toLowerCase();
    var classes = classSuffix(element);
    return tag + (classes ? '.' + classes : '');
  }

  function mutationTarget(record) {
    var target = record && record.target;
    if (target && target.nodeType === 3) target = target.parentElement;
    var attribute = record && record.type === 'attributes' ? (record.attributeName || 'attribute') : (record.type || 'mutation');
    return elementLabel(target) + '@' + attribute;
  }

  function safeStyle(element) {
    try { return getComputedStyle(element); } catch (e) { return null; }
  }

  function cssFilterInSVG(element) {
    if (!element || element.nodeType !== 1) return false;
    var node = element;
    while (node && node.nodeType === 1) {
      var style = safeStyle(node);
      if (node.hasAttribute('filter') || (style && style.filter && style.filter !== 'none')) return true;
      if (String(node.tagName).toLowerCase() === 'svg') break;
      node = node.parentElement;
    }
    return false;
  }

  function cssPropertyName(property) {
    return String(property || '').replace(/[A-Z]/g, function (letter) { return '-' + letter.toLowerCase(); });
  }

  function hasValue(list, value) {
    return list.indexOf(value) !== -1;
  }

  function unique(values) {
    var seen = Object.create(null);
    return values.filter(function (value) {
      if (seen[value]) return false;
      seen[value] = true;
      return true;
    });
  }

  function supportsPerformanceType(type) {
    try {
      if (!window.PerformanceObserver) return false;
      var supported = window.PerformanceObserver.supportedEntryTypes;
      return !supported || Array.prototype.indexOf.call(supported, type) !== -1;
    } catch (e) {
      return false;
    }
  }

  function observePerformance(type, callback) {
    if (!supportsPerformanceType(type)) return null;
    try {
      var observer = new PerformanceObserver(function (list) {
        try { callback(list.getEntries()); } catch (e) {}
      });
      try {
        observer.observe({ type: type, buffered: false });
      } catch (e) {
        observer.observe({ entryTypes: [type] });
      }
      return observer;
    } catch (e) {
      return null;
    }
  }

  function installPatches() {
    if (typeof nativeRAF === 'function') {
      window.requestAnimationFrame = function (callback) {
        var stack = callSite();
        rafScheduled += 1;
        count(rafStacks, stack, 0);
        var args = Array.prototype.slice.call(arguments, 1);
        var wrapped = function (timestamp) {
          rafCallbacks += 1;
          count(rafStacks, stack, 1);
          return callback.apply(this, [timestamp].concat(args));
        };
        return nativeRAF.apply(this, [wrapped].concat(args));
      };
    }
    if (typeof nativeTimeout === 'function') {
      window.setTimeout = function (callback, delay) {
        timeoutScheduled += 1;
        return nativeTimeout.apply(this, arguments);
      };
    }
    if (typeof nativeInterval === 'function') {
      window.setInterval = function (callback, delay) {
        intervalScheduled += 1;
        return nativeInterval.apply(this, arguments);
      };
    }
  }

  function collectAnimations() {
    var groups = Object.create(null);
    var list = [];
    try { list = document.getAnimations ? document.getAnimations() : []; } catch (e) { list = []; }
    list.forEach(function (animation) {
      if (animation.playState && animation.playState !== 'running') return;
      var effect = animation.effect;
      var target = effect && effect.target;
      var name = animation.animationName || animation.transitionProperty || 'anonymous animation';
      var key = String(name);
      if (!groups[key]) {
        groups[key] = {
          name: key,
          count: 0,
          iterations: null,
          infinite: false,
          durationMs: null,
          durationsMs: [],
          properties: [],
          targets: [],
          flags: [],
          mainThreadProperties: []
        };
      }
      var group = groups[key];
      group.count += 1;
      var timing = null;
      try { timing = effect && effect.getTiming ? effect.getTiming() : null; } catch (e) { timing = null; }
      var iterations = timing && timing.iterations;
      if (iterations === Infinity) {
        group.infinite = true;
        group.iterations = 'Infinity';
      } else if (iterations != null && !group.infinite) {
        if (group.iterations == null) group.iterations = iterations;
        else if (group.iterations !== iterations) group.iterations = 'varies';
      }
      var durationMs = timing && typeof timing.duration === 'number' ? timing.duration : null;
      if (durationMs != null) {
        if (group.durationMs == null) group.durationMs = durationMs;
        group.durationsMs.push(durationMs);
      }
      var properties = [];
      try {
        var keyframes = effect && effect.getKeyframes ? effect.getKeyframes() : [];
        keyframes.forEach(function (frame) {
          Object.keys(frame || {}).forEach(function (property) {
            if (!hasValue(['offset', 'computedOffset', 'easing', 'composite'], property)) properties.push(cssPropertyName(property));
          });
        });
      } catch (e) {}
      properties = unique(properties);
      group.properties = unique(group.properties.concat(properties));
      var targetText = elementLabel(target);
      if (targetText && !hasValue(group.targets, targetText)) group.targets.push(targetText);
      var flags = [];
      var compositorProperties = ['transform', 'opacity', 'translate', 'rotate', 'scale'];
      var mainThread = properties.filter(function (property) { return !hasValue(compositorProperties, property); });
      if (mainThread.length) flags.push('main-thread property');
      if (typeof SVGElement !== 'undefined' && target instanceof SVGElement) flags.push('SVG element (not composited in WebKit)');
      if (typeof SVGElement !== 'undefined' && target instanceof SVGElement && cssFilterInSVG(target)) flags.push('animated under an SVG/CSS filter: re-rasterized every frame');
      group.flags = unique(group.flags.concat(flags));
      group.mainThreadProperties = unique(group.mainThreadProperties.concat(mainThread));
    });
    return Object.keys(groups).map(function (key) {
      var group = groups[key];
      group.durationsMs = unique(group.durationsMs.map(function (value) { return Math.round(value * 100) / 100; }));
      return group;
    }).sort(function (a, b) { return b.count - a.count || a.name.localeCompare(b.name); });
  }

  function collectFilters() {
    var rows = [];
    var elements = document.querySelectorAll ? document.querySelectorAll('*') : [];
    for (var i = 0; i < elements.length; i += 1) {
      var element = elements[i];
      var style = safeStyle(element);
      if (!style) continue;
      var filter = style.filter || 'none';
      var backdrop = style.backdropFilter || style.webkitBackdropFilter || 'none';
      if (filter === 'none' && backdrop === 'none') continue;
      var rect = element.getBoundingClientRect();
      var width = Math.max(0, rect.width);
      var height = Math.max(0, rect.height);
      rows.push({
        target: elementLabel(element),
        filter: filter,
        backdropFilter: backdrop,
        width: Math.round(width * 100) / 100,
        height: Math.round(height * 100) / 100,
        area: Math.round(width * height * 100) / 100,
        smallBackdrop: backdrop !== 'none' && height < 40
      });
    }
    rows.sort(function (a, b) { return b.area - a.area; });
    return rows;
  }

  function collectCanvases() {
    var rows = [];
    var webGPUCanvasCount = 0;
    var canvases = document.querySelectorAll ? document.querySelectorAll('canvas') : [];
    for (var i = 0; i < canvases.length; i += 1) {
      var canvas = canvases[i];
      var contextType = null;
      var probeOrder = ['webgl2', 'webgl', '2d', 'bitmaprenderer'];
      for (var j = 0; j < probeOrder.length; j += 1) {
        try {
          if (canvas.getContext(probeOrder[j])) { contextType = probeOrder[j]; break; }
        } catch (e) {}
      }
      try {
        if (canvas.getContext('webgpu')) { contextType = 'webgpu'; webGPUCanvasCount += 1; }
      } catch (e) {}
      var rect = canvas.getBoundingClientRect();
      rows.push({
        target: elementLabel(canvas),
        width: canvas.width,
        height: canvas.height,
        cssWidth: Math.round(rect.width * 100) / 100,
        cssHeight: Math.round(rect.height * 100) / 100,
        contextType: contextType
      });
    }
    return { count: rows.length, webGPUAvailable: !!(navigator && navigator.gpu), anyWebGPUCanvas: webGPUCanvasCount > 0, webGPUCanvasCount: webGPUCanvasCount, canvases: rows };
  }

  function collectPage() {
    var reduced = false;
    try { reduced = !!(window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches); } catch (e) {}
    return {
      url: String(location.href),
      title: String(document.title || ''),
      hidden: !!document.hidden,
      nodeCount: document.getElementsByTagName ? document.getElementsByTagName('*').length : 0,
      devicePixelRatio: Number(window.devicePixelRatio || 1),
      innerWidth: Number(window.innerWidth || 0),
      innerHeight: Number(window.innerHeight || 0),
      prefersReducedMotion: reduced
    };
  }

  function number(value, places) {
    var factor = Math.pow(10, places || 0);
    return Math.round(value * factor) / factor;
  }

  function perSecond(value, elapsed) {
    return number(value / Math.max(elapsed, 0.001), 2);
  }

  function collectReport(elapsed) {
    var animationGroups = collectAnimations();
    var filterRows = collectFilters();
    var canvasReport = collectCanvases();
    var frameRows = Object.keys(rafStacks).map(function (stack) {
      return { stack: stack, callbacks: rafStacks[stack], perSecond: perSecond(rafStacks[stack], elapsed) };
    }).sort(function (a, b) { return b.callbacks - a.callbacks; });
    var mutationRows = Object.keys(mutationGroups).map(function (target) {
      return { target: target, mutations: mutationGroups[target], perSecond: perSecond(mutationGroups[target], elapsed) };
    }).sort(function (a, b) { return b.mutations - a.mutations; });
    var resourceRows = resources.slice().sort(function (a, b) { return b.durationMs - a.durationMs; });
    var pendingResources = [];
    try {
      (performance.getEntriesByType ? performance.getEntriesByType('resource') : []).forEach(function (entry) {
        if (Number(entry.startTime || 0) >= started && Number(entry.responseEnd || 0) === 0) {
          pendingResources.push({ name: String(entry.name || ''), initiatorType: String(entry.initiatorType || ''), startTimeMs: number(Number(entry.startTime || 0), 2) });
        }
      });
    } catch (e) {}
    var smallBackdropCount = filterRows.filter(function (row) { return row.smallBackdrop; }).length;
    var findings = [];
    function finding(severity, text) { findings.push({ severity: severity, text: text }); }
    function listed(values) { return values.join(', ') || 'unknown properties'; }
    animationGroups.forEach(function (group) {
      // A one-shot entrance that ends in a couple of seconds is not a standing cost; only
      // what keeps running (infinite, or a finite run longer than the sample) earns a finding.
      var standing = group.iterations === 'Infinity' || group.iterations === 'varies' ||
        (typeof group.iterations === 'number' && typeof group.durationMs === 'number' && group.iterations * group.durationMs > duration * 1000);
      if (!standing) return;
      if (hasValue(group.flags, 'animated under an SVG/CSS filter: re-rasterized every frame') && hasValue(group.flags, 'SVG element (not composited in WebKit)')) {
        finding(100, group.name + ' animates ' + listed(group.properties) + ' on ' + (group.targets[0] || 'an SVG element') + ' under an SVG/CSS filter — WebKit re-rasterizes the blurred surface every frame (Blink composites it).');
      }
      if (hasValue(group.properties, 'box-shadow')) {
        finding(95, group.name + ' animates box-shadow on ' + group.count + ' element' + (group.count === 1 ? '' : 's') + ' — not compositor-accelerated in WebKit.');
      } else if (hasValue(group.flags, 'main-thread property')) {
        finding(70, group.name + ' animates ' + listed(group.mainThreadProperties) + ' on ' + group.count + ' element' + (group.count === 1 ? '' : 's') + ' — a main-thread property in WebKit.');
      }
    });
    var distinctLoops = Object.keys(rafStacks).length;
    var callbackRate = perSecond(rafCallbacks, elapsed);
    if (distinctLoops > 10 || callbackRate > 30) {
      finding(60, distinctLoops + ' distinct requestAnimationFrame call-site loop' + (distinctLoops === 1 ? '' : 's') + ' recorded at ~' + callbackRate + '/s' + (document.hidden ? ' while document.hidden (rAF may be throttled).' : '.'));
    }
    if (smallBackdropCount > 10) {
      finding(55, smallBackdropCount + ' elements carry backdrop-filter and are under 40px tall.');
    }
    if (mutations / Math.max(elapsed, 0.001) > 50) {
      finding(45, 'DOM churn recorded ' + perSecond(mutations, elapsed) + ' mutations/s; top targets are ' + mutationRows.slice(0, 3).map(function (row) { return row.target; }).join(', ') + '.');
    }
    if (longTaskCount > 0) {
      finding(35, longTaskCount + ' long task' + (longTaskCount === 1 ? '' : 's') + ' occupied ' + number(longTaskMs, 1) + 'ms during the sample.');
    }
    findings.sort(function (a, b) { return b.severity - a.severity || a.text.localeCompare(b.text); });
    if (!findings.length) findings.push({ severity: 0, text: 'Looks fine — no WebKit-specific hot-tab heuristics tripped.' });

    var raw = {
      windowSeconds: number(elapsed, 2),
      heuristics: findings,
      frameLoop: {
        scheduled: rafScheduled,
        callbacks: rafCallbacks,
        callbacksPerSecond: callbackRate,
        distinctConcurrentLoops: distinctLoops,
        callSites: frameRows.slice(0, rowLimit)
      },
      domChurn: {
        mutations: mutations,
        mutationsPerSecond: perSecond(mutations, elapsed),
        targets: mutationRows.slice(0, rowLimit)
      },
      animations: {
        count: animationGroups.reduce(function (total, group) { return total + group.count; }, 0),
        groups: animationGroups.slice(0, rowLimit)
      },
      filters: {
        count: filterRows.length,
        manySmallBackdrop: smallBackdropCount > 10,
        smallBackdropCount: smallBackdropCount,
        elements: filterRows.slice(0, rowLimit)
      },
      canvases: canvasReport,
      timers: {
        setTimeoutScheduled: timeoutScheduled,
        setTimeoutScheduledPerSecond: perSecond(timeoutScheduled, elapsed),
        setIntervalScheduled: intervalScheduled,
        setIntervalScheduledPerSecond: perSecond(intervalScheduled, elapsed),
        longTaskSupported: longTaskObserver !== null,
        longTaskCount: longTaskCount,
        longTaskTotalMs: number(longTaskMs, 1)
      },
      resources: {
        observed: resourceRows.length,
        slow: resourceRows.slice(0, rowLimit),
        pending: pendingResources.slice(0, rowLimit)
      },
      page: collectPage()
    };
    return raw;
  }

  function markdown(report) {
    var lines = [];
    lines.push('# Summary');
    report.heuristics.forEach(function (item) { lines.push('- ' + item.text); });
    lines.push('');
    lines.push('# Frame loop');
    lines.push('- ' + report.frameLoop.callbacks + ' callbacks (' + report.frameLoop.callbacksPerSecond + '/s); ' + report.frameLoop.distinctConcurrentLoops + ' distinct concurrent loop stack' + (report.frameLoop.distinctConcurrentLoops === 1 ? '' : 's') + '; ' + report.frameLoop.scheduled + ' requests scheduled.');
    if (report.frameLoop.callSites.length) {
      report.frameLoop.callSites.slice(0, rowLimit).forEach(function (row) { lines.push('- ' + row.callbacks + ' (' + row.perSecond + '/s): `' + row.stack + '`'); });
    } else lines.push('- No requestAnimationFrame callbacks observed.');
    lines.push('');
    lines.push('# DOM churn');
    lines.push('- ' + report.domChurn.mutations + ' mutations (' + report.domChurn.mutationsPerSecond + '/s).');
    if (report.domChurn.targets.length) report.domChurn.targets.slice(0, rowLimit).forEach(function (row) { lines.push('- ' + row.mutations + ' (' + row.perSecond + '/s): `' + row.target + '`'); });
    else lines.push('- No mutations observed.');
    lines.push('');
    lines.push('# Animations');
    lines.push('- ' + report.animations.count + ' running animation' + (report.animations.count === 1 ? '' : 's') + ' at the end of the window.');
    if (report.animations.groups.length) report.animations.groups.slice(0, rowLimit).forEach(function (group) {
      lines.push('- **' + group.name + '** ×' + group.count + '; iterations ' + (group.iterations == null ? 'unknown' : group.iterations) + '; duration ' + (group.durationMs == null ? 'unknown' : group.durationMs + 'ms') + '; properties: ' + (group.properties.join(', ') || 'unknown') + '; target: ' + (group.targets[0] || 'unknown') + (group.flags.length ? '; FLAGS: ' + group.flags.join(' | ') : ''));
    });
    else lines.push('- No running animations found.');
    lines.push('');
    lines.push('# Filters');
    lines.push('- ' + report.filters.count + ' elements with CSS filter or backdrop-filter; ' + report.filters.smallBackdropCount + ' small backdrop-filter elements under 40px tall.');
    if (report.filters.elements.length) report.filters.elements.slice(0, rowLimit).forEach(function (row) { lines.push('- ' + row.target + ' ' + row.width + '×' + row.height + '; filter: `' + row.filter + '`; backdrop-filter: `' + row.backdropFilter + '`'); });
    else lines.push('- No CSS filters or backdrop filters found.');
    lines.push('');
    lines.push('# Canvases');
    lines.push('- ' + report.canvases.count + ' canvas' + (report.canvases.count === 1 ? '' : 'es') + '; navigator.gpu: ' + (report.canvases.webGPUAvailable ? 'present' : 'absent') + '; any webgpu canvas context: ' + (report.canvases.anyWebGPUCanvas ? 'yes' : 'no') + '.');
    report.canvases.canvases.slice(0, rowLimit).forEach(function (row) { lines.push('- ' + row.target + ' ' + row.width + '×' + row.height + ' (' + row.cssWidth + '×' + row.cssHeight + ' CSS px); context: ' + (row.contextType || 'none')); });
    lines.push('');
    lines.push('# Timers/long tasks');
    lines.push('- setTimeout scheduled: ' + report.timers.setTimeoutScheduled + ' (' + report.timers.setTimeoutScheduledPerSecond + '/s); setInterval scheduled: ' + report.timers.setIntervalScheduled + ' (' + report.timers.setIntervalScheduledPerSecond + '/s).');
    lines.push('- Long tasks: ' + report.timers.longTaskCount + ', ' + report.timers.longTaskTotalMs + 'ms total' + (report.timers.longTaskSupported ? '' : ' (PerformanceObserver longtask unavailable)') + '.');
    lines.push('');
    lines.push('# Resources');
    lines.push('- ' + report.resources.observed + ' resource entries observed during the window; pending entries at capture: ' + report.resources.pending.length + '.');
    report.resources.slow.slice(0, rowLimit).forEach(function (row) { lines.push('- ' + row.durationMs + 'ms — `' + row.name + '` (' + (row.initiatorType || 'unknown') + ')'); });
    report.resources.pending.slice(0, rowLimit).forEach(function (row) { lines.push('- pending — `' + row.name + '` (' + (row.initiatorType || 'unknown') + ')'); });
    if (!report.resources.slow.length && !report.resources.pending.length) lines.push('- No resource entries observed.');
    lines.push('');
    lines.push('# Page');
    lines.push('- URL: ' + report.page.url);
    lines.push('- Nodes: ' + report.page.nodeCount + '; hidden: ' + report.page.hidden + '; devicePixelRatio: ' + report.page.devicePixelRatio + '; viewport: ' + report.page.innerWidth + '×' + report.page.innerHeight + '.');
    lines.push('- prefers-reduced-motion: ' + report.page.prefersReducedMotion + '; sample window: ' + report.windowSeconds + 's.');
    if (report.page.hidden) lines.push('- The document is hidden/off-screen; requestAnimationFrame may be throttled, so frame rates are a lower bound.');
    return lines.join('\n');
  }

  function finishObservers() {
    if (mutationObserver) mutationObserver.disconnect();
    if (longTaskObserver) longTaskObserver.disconnect();
    if (resourceObserver) resourceObserver.disconnect();
  }

  installPatches();
  try {
    if (window.MutationObserver && document.documentElement) {
      mutationObserver = new MutationObserver(function (records) {
        records.forEach(function (record) {
          mutations += 1;
          count(mutationGroups, mutationTarget(record), 1);
        });
      });
      mutationObserver.observe(document.documentElement, { subtree: true, childList: true, attributes: true, characterData: true });
    }
    longTaskObserver = observePerformance('longtask', function (entries) {
      entries.forEach(function (entry) { longTaskCount += 1; longTaskMs += Number(entry.duration || 0); });
    });
    resourceObserver = observePerformance('resource', function (entries) {
      entries.forEach(function (entry) {
        if (Number(entry.startTime || 0) < started) return;
        resources.push({ name: String(entry.name || ''), initiatorType: String(entry.initiatorType || ''), durationMs: number(Number(entry.duration || 0), 2), startTimeMs: number(Number(entry.startTime || 0), 2), transferSize: Number(entry.transferSize || 0) });
      });
    });
    await new Promise(function (resolve) { nativeTimeout(resolve, duration * 1000); });
    await new Promise(function (resolve) { nativeTimeout(resolve, 0); });
    var elapsed = Math.max(0.001, (performance.now() - started) / 1000);
    var report = collectReport(elapsed);
    finishObservers();
    if (outputFormat === 'json') return report;
    return markdown(report);
  } finally {
    finishObservers();
    if (typeof nativeRAF === 'function') window.requestAnimationFrame = nativeRAF;
    if (typeof nativeTimeout === 'function') window.setTimeout = nativeTimeout;
    if (typeof nativeInterval === 'function') window.setInterval = nativeInterval;
  }
})()
}
