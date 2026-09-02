// Raises the browser tab already showing the dashboard. Run by the macOS app's
// Dock-click handler (src/commands/app.rs); prints "ok", "blocked" or "none".
//
// JXA rather than AppleScript: it resolves each browser's terminology at run
// time, so this still runs on a Mac without Chrome installed — an AppleScript
// `using terms from` block fails to *compile* there. Chromium forks all answer
// Chrome's vocabulary; Safari needs its own. A browser missing from the list
// degrades to the caller's later tiers, so staleness here is cosmetic.
function run(argv) {
  var url = argv[0];
  var blocked = false;

  // Specifiers are lazy: the Apple event fires when a count or a URL is read,
  // not when `app.windows` is built, so those reads are what throw when a
  // browser is unresponsive or Automation was denied. Such throws belong to the
  // caller, which decides the whole browser's fate; only per-window and per-tab
  // oddities are swallowed here.
  function focusIn(app, setActive) {
    var wins = app.windows;
    var windowCount = wins.length;
    for (var w = 0; w < windowCount; w++) {
      var win = wins[w];
      var urls;
      try {
        // One event for every URL in the window, rather than one per tab.
        urls = win.tabs.url();
      } catch (e) {
        // No tabs to read: an inspector or settings window.
        continue;
      }
      for (var t = 0; t < urls.length; t++) {
        if (!urls[t] || urls[t].indexOf(url) !== 0) continue;
        try {
          setActive(win, t);
        } catch (e) {
          // A fork that exposes tabs but refuses the selection: keep looking so
          // the caller still gets a chance to fall through to its own tiers.
          continue;
        }
        // A minimized window is exactly the state someone clicks the Dock in.
        // The two spellings are the Chromium and Safari names for it.
        try {
          win.minimized = false;
        } catch (e) {}
        try {
          win.miniaturized = false;
        } catch (e) {}
        try {
          win.index = 1;
        } catch (e) {}
        try {
          app.activate();
        } catch (e) {}
        return true;
      }
    }
    return false;
  }

  // -1743 is "not authorized to send Apple events" — the user denied the
  // Automation prompt, which is worth reporting. Anything else is a browser
  // that is not installed or not answering, so just try the next one.
  function search(bundleId, setActive) {
    var app;
    try {
      app = Application(bundleId);
      // Answered by the scripting runtime: it neither launches the app nor
      // sends an Apple event, so an unauthorized browser still reaches focusIn.
      if (!app.running()) return false;
    } catch (e) {
      // Only expected for a browser that is not installed, but classify anyway
      // in case a future macOS routes `running()` through TCC too.
      if (e && e.errorNumber === -1743) blocked = true;
      return false;
    }
    try {
      return focusIn(app, setActive);
    } catch (e) {
      if (e && e.errorNumber === -1743) blocked = true;
      return false;
    }
  }

  var chromium = [
    "com.google.Chrome",
    "com.google.Chrome.beta",
    "com.google.Chrome.dev",
    "com.google.Chrome.canary",
    "com.brave.Browser",
    "com.microsoft.edgemac",
    "company.thebrowser.Browser",
    "org.chromium.Chromium",
    "com.vivaldi.Vivaldi",
    "com.operasoftware.Opera",
  ];
  function selectChromiumTab(win, index) {
    win.activeTabIndex = index + 1;
  }
  for (var i = 0; i < chromium.length; i++) {
    if (search(chromium[i], selectChromiumTab)) return "ok";
  }

  if (
    search("com.apple.Safari", function (win, index) {
      win.currentTab = win.tabs[index];
    })
  ) {
    return "ok";
  }

  return blocked ? "blocked" : "none";
}
