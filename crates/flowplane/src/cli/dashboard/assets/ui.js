// Shared dashboard interaction helpers. This module is deliberately DOM-only:
// htmx remains the sole owner of network acquisition.
(function (global) {
  "use strict";

  function paginate(items, query, requestedPage, pageSize, searchableText) {
    const normalized = query.trim().toLowerCase();
    const matches = items.filter(function (item) {
      return !normalized || searchableText(item).toLowerCase().includes(normalized);
    });
    const maxPage = Math.max(1, Math.ceil(matches.length / pageSize));
    const page = Math.min(Math.max(0, requestedPage), maxPage - 1);
    const start = page * pageSize;
    const visible = matches.slice(start, start + pageSize);
    return {
      matches: matches,
      visible: visible,
      page: page,
      maxPage: maxPage,
      start: matches.length ? start + 1 : 0,
      end: Math.min(start + pageSize, matches.length),
      total: matches.length
    };
  }

  function selectTab(tabs, panels, name, focus) {
    let active = null;
    tabs.forEach(function (tab) {
      const selected = tab.dataset.uiTab === name;
      tab.classList.toggle("is-active", selected);
      tab.setAttribute("aria-selected", selected ? "true" : "false");
      tab.tabIndex = selected ? 0 : -1;
      if (selected) active = tab;
    });
    panels.forEach(function (panel) {
      const selected = panel.dataset.uiTabPanel === name;
      panel.classList.toggle("is-active", selected);
      panel.hidden = !selected;
    });
    if (focus && active) active.focus();
    return active;
  }

  global.FlowplaneUI = Object.freeze({
    paginate: paginate,
    selectTab: selectTab
  });
})(window);
