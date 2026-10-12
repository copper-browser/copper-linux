// from Sources/Search/Curtain.swift:148 Veiling.style
// user script at document start (main frame) and eval on update; the CSS is an argument, not spliced into a template literal.
({ css }) => {
  var sheet = document.getElementById('office-veil');
  if (!sheet) {
    sheet = document.createElement('style');
    sheet.id = 'office-veil';
    (document.head || document.documentElement).appendChild(sheet);
  }
  sheet.textContent = css;
}
