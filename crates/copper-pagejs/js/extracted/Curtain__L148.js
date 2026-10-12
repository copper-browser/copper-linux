// from Sources/Search/Curtain.swift:148 Veiling.style
(function () {
  var sheet = document.getElementById('office-veil');
  if (!sheet) {
    sheet = document.createElement('style');
    sheet.id = 'office-veil';
    (document.head || document.documentElement).appendChild(sheet);
  }
  sheet.textContent = `/*INTERP: escaped*/`;
})();
