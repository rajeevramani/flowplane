// APIs master/detail, filter and display paging. DOM-only by design: htmx owns the one
// detail request, while this script owns visibility and ARIA state.
(function () {
  "use strict";

  const PAGE_SIZE = 25;

  function detailFor(button) {
    return document.getElementById(button.getAttribute("aria-controls"));
  }

  function collapse(button) {
    var detail = detailFor(button);
    button.setAttribute("aria-expanded", "false");
    button.closest("tr").classList.remove("selected");
    if (detail) detail.hidden = true;
  }

  function expand(button) {
    var detail = detailFor(button);
    button.setAttribute("aria-expanded", "true");
    button.closest("tr").classList.add("selected");
    if (detail) detail.hidden = false;
    if (button.getAttribute("data-loaded") !== "true") {
      button.setAttribute("data-loaded", "true");
      button.dispatchEvent(new CustomEvent("api-expand", { bubbles: false }));
    }
  }

  function initialize(panel) {
    if (!panel || panel.getAttribute("data-apis-ready") === "true") return;
    var filter = panel.querySelector("[data-api-filter]");
    var previous = panel.querySelector("[data-api-prev]");
    var next = panel.querySelector("[data-api-next]");
    var range = panel.querySelector("[data-api-range]");
    var rows = Array.prototype.slice.call(panel.querySelectorAll("tr[data-api-row]"));
    if (!filter || !previous || !next || !range) return;

    panel.setAttribute("data-apis-ready", "true");
    let page = 0;

    function render() {
      var query = filter.value.trim().toLowerCase();
      var matches = rows.filter(function (row) {
        return row.dataset.search.toLowerCase().indexOf(query) !== -1;
      });
      var pages = Math.max(1, Math.ceil(matches.length / PAGE_SIZE));
      if (page >= pages) page = pages - 1;
      var start = page * PAGE_SIZE;
      var end = Math.min(start + PAGE_SIZE, matches.length);

      rows.forEach(function (row) {
        var visibleIndex = matches.indexOf(row);
        var visible = visibleIndex >= start && visibleIndex < end;
        row.hidden = !visible;
        var button = row.querySelector("button[data-api-disclosure]");
        var detail = button && detailFor(button);
        if (!visible && button) collapse(button);
        if (detail && !visible) detail.hidden = true;
      });

      previous.disabled = page === 0;
      next.disabled = page + 1 >= pages || matches.length === 0;
      range.textContent = matches.length === 0
        ? "0–0 of 0"
        : String(start + 1) + "–" + String(end) + " of " + String(matches.length);
    }

    filter.addEventListener("input", function () {
      page = 0;
      render();
    });
    previous.addEventListener("click", function () {
      if (page > 0) page -= 1;
      render();
    });
    next.addEventListener("click", function () {
      page += 1;
      render();
    });
    render();
  }

  document.addEventListener("click", function (event) {
    var button = event.target.closest && event.target.closest("button[data-api-disclosure]");
    if (!button) return;
    var panel = button.closest("[data-apis-panel]");
    panel.querySelectorAll("button[data-api-disclosure][aria-expanded=\"true\"]")
      .forEach(function (other) {
        if (other !== button) collapse(other);
      });
    if (button.getAttribute("aria-expanded") === "true") collapse(button);
    else expand(button);
  });

  document.addEventListener("DOMContentLoaded", function () {
    document.querySelectorAll("[data-apis-panel]").forEach(initialize);
  });
  document.addEventListener("htmx:afterSwap", function (event) {
    if (event.target.matches && event.target.matches("[data-apis-panel]")) initialize(event.target);
    event.target.querySelectorAll && event.target.querySelectorAll("[data-apis-panel]").forEach(initialize);
  });
})();
