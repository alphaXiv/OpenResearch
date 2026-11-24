// The overlay a page shows while OpenResearch restarts into an update. The page that starts the
// restart draws it and saves a record of it; after the reload, the next build's index.html runs this
// file before its first paint and draws the same overlay from that record, so the swap never shows.
// The record and the overlay's look are a contract between builds: arc/desktop.md, "Keeping the
// window through an update". Plain ES5, since it runs before anything checks the browser.
(function () {
  var KEY = "orx:applying-update";
  var ID = "orx-update";
  // A record older than a restart can take is left from a page that never finished.
  var STALE_MS = 10 * 60 * 1000;
  var SPIN_MS = 800;
  var FADE_MS = 150;
  // Windows has no caption buttons of its own: the page draws them (WindowControls.tsx), and so,
  // over the page, does the overlay. ChromeMinimize, ChromeMaximize, ChromeRestore, ChromeClose.
  var CAPTIONS = "__ORX_WIN_TITLEBAR__" in window;
  var GLYPH = { minimize: "\uE921", maximize: "\uE922", restore: "\uE923", close: "\uE8BB" };
  // Their names, which the page sets in its language and the record carries to the next page.
  var labels = null;

  function child(parent, tag, className) {
    var node = document.createElement(tag);
    node.className = className;
    parent.appendChild(node);
    return node;
  }

  function app() {
    return document.getElementById("root");
  }

  function create() {
    var root = document.createElement("div");
    root.id = ID;
    root.className = "fixed inset-0 z-1000 flex items-center justify-center p-5 bg-background motion-safe:transition-opacity motion-safe:duration-150 ease-standard";
    var card = child(root, "div", "w-120 max-w-full rounded-xl border border-border bg-background p-6 shadow-modal outline-none");
    card.id = ID + "-dialog";
    card.tabIndex = -1;
    card.setAttribute("role", "alertdialog");
    card.setAttribute("aria-modal", "true");
    card.setAttribute("aria-labelledby", ID + "-title");
    card.setAttribute("aria-describedby", ID + "-body");
    child(card, "h2", "m-0 text-xl font-medium text-text").id = ID + "-title";
    var line = child(card, "p", "m-0 mt-2 flex items-center gap-2 text-sm text-subtext");
    // Phased by the clock, so the spinner on the next page carries on where this one was.
    var spinner = child(line, "span", "spinner size-3.5 shrink-0 animate-[spin_0.8s_linear_infinite] rounded-full border-2 border-border border-t-subtext");
    spinner.id = ID + "-spinner";
    spinner.style.animationDelay = -(Date.now() % SPIN_MS) + "ms";
    child(line, "span", "min-w-0 break-words").id = ID + "-body";
    child(card, "div", "mt-4 empty:hidden").id = ID + "-details";
    child(card, "div", "mt-6 flex justify-end gap-2.5 empty:hidden").id = ID + "-actions";

    if (CAPTIONS) {
      var row = child(root, "div", "native-window-controls fixed end-0 top-0 flex");
      row.id = ID + "-captions";
      caption(row, "minimize", "hover:bg-panel");
      caption(row, "zoom", "hover:bg-panel");
      caption(row, "close", "hover:bg-accent-red hover:text-white");
    }
    document.body.appendChild(root);
    return root;
  }

  function caption(row, message, hover) {
    var button = child(row, "button", "inline-flex h-8 w-11.5 items-center justify-center text-xs text-text font-['Segoe_Fluent_Icons','Segoe_MDL2_Assets'] " + hover);
    button.type = "button";
    // Native caption buttons aren't in the Tab order.
    button.tabIndex = -1;
    button.setAttribute("data-titlebar", message);
    button.addEventListener("click", function () {
      if (window.ipc) window.ipc.postMessage("titlebar:" + message);
    });
  }

  /** Names the caption buttons, and shows the maximize one as the window is. */
  function syncCaptions() {
    var row = document.getElementById(ID + "-captions");
    if (!row) return;
    row.hidden = !labels;
    if (!labels) return;
    var maximized = document.documentElement.hasAttribute("data-maximized");
    var glyphs = { minimize: GLYPH.minimize, zoom: maximized ? GLYPH.restore : GLYPH.maximize, close: GLYPH.close };
    var names = { minimize: labels.minimize, zoom: maximized ? labels.restore : labels.maximize, close: labels.close };
    for (var i = 0; i < row.children.length; i += 1) {
      var button = row.children[i];
      var message = button.getAttribute("data-titlebar");
      button.textContent = glyphs[message];
      button.title = names[message];
      button.setAttribute("aria-label", names[message]);
    }
  }

  /** Draws `{ title, body, busy }`, fading in over the page when `fade` and nothing was showing. */
  function show(state, fade) {
    var root = document.getElementById(ID);
    var fresh = !root;
    if (!root) root = create();
    root.removeAttribute("data-leaving");
    document.getElementById(ID + "-title").textContent = state.title;
    document.getElementById(ID + "-body").textContent = state.body;
    document.getElementById(ID + "-spinner").hidden = !state.busy;
    document.getElementById(ID + "-dialog").setAttribute("aria-busy", state.busy ? "true" : "false");
    syncCaptions();
    var page = app();
    if (page) page.inert = true;
    if (fresh && fade) {
      root.classList.add("opacity-0");
      requestAnimationFrame(function () {
        requestAnimationFrame(function () {
          root.classList.remove("opacity-0");
        });
      });
    } else {
      root.classList.remove("opacity-0");
    }
    return root;
  }

  function hide() {
    var root = document.getElementById(ID);
    if (!root || root.hasAttribute("data-leaving")) return;
    root.setAttribute("data-leaving", "");
    root.classList.add("opacity-0");
    var page = app();
    if (page) page.inert = false;
    setTimeout(function () {
      if (root.hasAttribute("data-leaving")) root.remove();
    }, FADE_MS);
  }

  /** The record the restarting page saved, if it is recent. */
  function pending() {
    try {
      var record = JSON.parse(sessionStorage.getItem(KEY) || "null");
      if (record && typeof record.title === "string" && typeof record.body === "string" && Date.now() - record.at < STALE_MS) return record;
    } catch (e) {}
    return null;
  }

  function clear() {
    try {
      sessionStorage.removeItem(KEY);
    } catch (e) {}
  }

  window.orxUpdateOverlay = {
    /** Saves `{ from, to, title, body }` for the next page and fades the overlay in over this one. */
    begin: function (record) {
      try {
        sessionStorage.setItem(KEY, JSON.stringify({ from: record.from, to: record.to, title: record.title, body: record.body, captions: labels, at: Date.now() }));
      } catch (e) {}
      show({ title: record.title, body: record.body, busy: true }, true);
    },
    pending: pending,
    show: show,
    /** The overlay's button row, for the page to render into. */
    actions: function () {
      return document.getElementById(ID + "-actions");
    },
    /** The space under the body, for the page to render into. */
    details: function () {
      return document.getElementById(ID + "-details");
    },
    /** Forgets the record and fades the overlay out. */
    end: function () {
      clear();
      hide();
    },

    /** Sets `{ minimize, maximize, restore, close }`, the caption buttons' names, on Windows. */
    captions: function (names) {
      labels = names;
      syncCaptions();
    },
  };

  if (CAPTIONS) new MutationObserver(syncCaptions).observe(document.documentElement, { attributes: true, attributeFilter: ["data-maximized"] });

  var record = pending();
  if (record) {
    labels = record.captions || null;
    show({ title: record.title, body: record.body, busy: true }, false);
  }
  else clear();
})();
