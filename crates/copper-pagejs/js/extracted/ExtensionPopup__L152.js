// from Sources/Search/ExtensionPopup.swift:152 ExtensionPopup.preferred
() => {
  const d = document.documentElement;
  if (!d) return null;
  const m = window.__searchSizing || (window.__searchSizing = {});
  const saved = d.getAttribute("style");
  const back = () => saved === null ? d.removeAttribute("style") : d.setAttribute("style", saved);
  const box = d.getBoundingClientRect();
  // A width the page sets for itself shows as one the view doesn't
  // have; one equal to the view is either filling it or the width it
  // was given last time, remembered.
  let w;
  if (Math.abs(box.width - innerWidth) > 1) w = m.w = box.width;
  else if (m.w && Math.abs(m.w - innerWidth) <= 1) w = m.w;
  else {
    d.style.setProperty("width", "min-content", "important");
    const narrowest = d.getBoundingClientRect().width;
    back();
    w = narrowest >= 100 ? narrowest : Math.max(narrowest, d.scrollWidth);
  }
  w = Math.min(800, Math.max(25, Math.ceil(w)));
  d.style.setProperty("width", w + "px", "important");
  let h = d.getBoundingClientRect().height;
  if (Math.abs(h - innerHeight) > 1) m.h = h;
  else if (m.h && Math.abs(m.h - innerHeight) <= 1) h = m.h;
  else {
    d.style.setProperty("height", "auto", "important");
    d.style.setProperty("min-height", "0", "important");
    h = d.getBoundingClientRect().height;
  }
  back();
  return [w, Math.min(600, Math.max(25, Math.ceil(h)))];
}
