// APIs master/detail, filter and display paging. DOM-only by design: htmx owns the one
// detail request, while this script owns visibility and ARIA state.
(function () {
  "use strict";

  const PAGE_SIZE = 25;
  const UI = window.FlowplaneUI;

  function detailFor(button) {
    return document.getElementById(button.getAttribute("aria-controls"));
  }

  function collapse(button) {
    var detail = detailFor(button);
    button.setAttribute("aria-expanded", "false");
    button.closest("tr").classList.remove("is-selected");
    if (detail) detail.hidden = true;
  }

  function expand(button) {
    var detail = detailFor(button);
    button.setAttribute("aria-expanded", "true");
    button.closest("tr").classList.add("is-selected");
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
      var result = UI.paginate(rows, filter.value, page, PAGE_SIZE, function (row) {
        return row.dataset.search;
      });
      page = result.page;

      rows.forEach(function (row) {
        var visible = result.visible.indexOf(row) !== -1;
        row.hidden = !visible;
        var button = row.querySelector("button[data-api-disclosure]");
        var detail = button && detailFor(button);
        if (!visible && button) collapse(button);
        if (detail && !visible) detail.hidden = true;
      });

      previous.disabled = page === 0;
      next.disabled = page + 1 >= result.maxPage || result.total === 0;
      range.textContent = result.total === 0
        ? "0–0 of 0"
        : String(result.start) + "–" + String(result.end) + " of " + String(result.total);
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
