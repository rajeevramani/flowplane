// Resources view ownership: DOM state only. htmx remains the sole network owner.
(function () {
  "use strict";

  const PAGE_SIZE = 25;
  const UI = window.FlowplaneUI;

  function selectView(shell, name, focus) {
    const tabs = Array.from(shell.querySelectorAll("[data-ui-tab]"));
    const panels = Array.from(shell.querySelectorAll("[data-ui-tab-panel]"));
    UI.selectTab(tabs, panels, name, focus);
    panels.forEach(function (panel) {
      const active = panel.dataset.uiTabPanel === name;
      if (active && name === "tables" && !panel.dataset.loaded) {
        panel.dataset.loaded = "true";
        panel.querySelectorAll('[hx-trigger="resources-tables once"]').forEach(function (loader) {
          loader.dispatchEvent(new CustomEvent("resources-tables"));
        });
      }
    });
  }

  function initShell(shell) {
    if (!shell || shell.dataset.resourcesReady) return;
    shell.dataset.resourcesReady = "true";
    const tabs = Array.from(shell.querySelectorAll("[data-ui-tab]"));
    tabs.forEach(function (tab, index) {
      tab.addEventListener("click", function () {
        selectView(shell, tab.dataset.uiTab, false);
      });
      tab.addEventListener("keydown", function (event) {
        let next = index;
        if (event.key === "ArrowRight") next = (index + 1) % tabs.length;
        else if (event.key === "ArrowLeft") next = (index + tabs.length - 1) % tabs.length;
        else if (event.key === "Home") next = 0;
        else if (event.key === "End") next = tabs.length - 1;
        else return;
        event.preventDefault();
        selectView(shell, tabs[next].dataset.uiTab, true);
      });
    });
  }

  function setExpanded(row, expanded) {
    row.classList.toggle("is-open", expanded);
    const button = row.querySelector("[data-topology-disclosure]");
    if (button) button.setAttribute("aria-expanded", expanded ? "true" : "false");
  }

  function initTopology(panel) {
    if (!panel || panel.dataset.topologyReady) return;
    panel.dataset.topologyReady = "true";
    const rows = Array.from(panel.querySelectorAll("[data-topology-row]"));
    const filter = panel.querySelector("[data-topology-filter]");
    const range = panel.querySelector("[data-topology-range]");
    const previous = panel.querySelector("[data-topology-prev]");
    const next = panel.querySelector("[data-topology-next]");
    const empty = panel.querySelector("[data-topology-empty]");
    let page = 0;
    let matchingRows = rows;

    function render() {
      const result = UI.paginate(rows, filter ? filter.value : "", page, PAGE_SIZE, function (row) {
        return row.textContent;
      });
      page = result.page;
      matchingRows = result.matches;
      rows.forEach(function (row) { row.hidden = result.visible.indexOf(row) === -1; });
      if (range) range.textContent = result.total
        ? result.start + "–" + result.end + " of " + result.total + " listeners"
        : "0–0 of 0 listeners";
      if (previous) previous.disabled = page === 0;
      if (next) next.disabled = page >= result.maxPage - 1 || result.total === 0;
      if (empty) empty.hidden = result.total !== 0;
    }

    rows.forEach(function (row, index) {
      setExpanded(row, index < 2);
      const disclosure = row.querySelector("[data-topology-disclosure]");
      if (disclosure) disclosure.addEventListener("click", function () {
        setExpanded(row, disclosure.getAttribute("aria-expanded") !== "true");
      });
    });
    if (filter) filter.addEventListener("input", function () { page = 0; render(); });
    if (previous) previous.addEventListener("click", function () { if (page > 0) { page -= 1; render(); } });
    if (next) next.addEventListener("click", function () { page += 1; render(); });
    const expand = panel.querySelector("[data-topology-expand]");
    const collapse = panel.querySelector("[data-topology-collapse]");
    if (expand) expand.addEventListener("click", function () {
      matchingRows.forEach(function (row) { setExpanded(row, true); });
    });
    if (collapse) collapse.addEventListener("click", function () {
      matchingRows.forEach(function (row) { setExpanded(row, false); });
    });
    render();
  }

  function clearHighlights() {
    document.querySelectorAll("[data-cluster].is-highlighted").forEach(function (node) {
      node.classList.remove("is-highlighted");
    });
  }

  document.addEventListener("mouseover", function (event) {
    const target = event.target && event.target.closest && event.target.closest("[data-cluster]");
    if (!target) return;
    const name = target.getAttribute("data-cluster");
    clearHighlights();
    document.querySelectorAll("[data-cluster]").forEach(function (node) {
      if (node.getAttribute("data-cluster") === name) node.classList.add("is-highlighted");
    });
  });
  document.addEventListener("mouseout", function (event) {
    if (event.target && event.target.closest && event.target.closest("[data-cluster]")) clearHighlights();
  });

  document.addEventListener("DOMContentLoaded", function () {
    initShell(document.querySelector("[data-resources-shell]"));
    initTopology(document.querySelector("[data-topology-panel]"));
  });
  document.addEventListener("htmx:afterSwap", function (event) {
    const target = event.detail && event.detail.target;
    if (!target) return;
    initTopology(target.matches && target.matches("[data-topology-panel]")
      ? target : target.querySelector && target.querySelector("[data-topology-panel]"));
  });
})();
