// Topology hover highlight (fpv2-cxw.2): hovering any cluster chip highlights every
// chip for the SAME cluster across all chains. Served same-origin because the CSP
// (default-src 'self') forbids inline script.
(function () {
  "use strict";
  function clear() {
    document.querySelectorAll("[data-cluster].hl").forEach(function (n) {
      n.classList.remove("hl");
    });
  }
  document.addEventListener("mouseover", function (e) {
    var el = e.target && e.target.closest && e.target.closest("[data-cluster]");
    if (!el) return;
    var name = el.getAttribute("data-cluster");
    clear();
    document.querySelectorAll("[data-cluster]").forEach(function (n) {
      if (n.getAttribute("data-cluster") === name) n.classList.add("hl");
    });
  });
  document.addEventListener("mouseout", function (e) {
    if (e.target && e.target.closest && e.target.closest("[data-cluster]")) clear();
  });
})();
