import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import ts from "typescript";

const require = createRequire(import.meta.url);
const messages = { m: new Proxy({}, { get: (_, name) => () => String(name) }) };

function load(file, mocks, globals = {}) {
  const source = readFileSync(new URL(`../src/${file}`, import.meta.url), "utf8");
  const code = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
  }).outputText;
  const exports = {};
  new Function("require", "exports", ...Object.keys(globals), code)(
    (name) => mocks[name] ?? require(name), exports, ...Object.values(globals),
  );
  return exports;
}

function hooks() {
  const values = [];
  let cursor = 0;
  return {
    reset: () => { cursor = 0; },
    react: {
      ...React,
      useEffect: () => {},
      useRef: () => ({ current: null }),
      useState: (initial) => {
        const index = cursor++;
        if (!(index in values)) values[index] = initial;
        return [values[index], (value) => { values[index] = value; }];
      },
    },
  };
}

function nodes(tree) {
  if (!React.isValidElement(tree)) return [];
  return [tree, ...React.Children.toArray(tree.props.children).flatMap(nodes)];
}

for (const corner of [true, false]) {
  test(`local ${corner ? "home" : "sidebar"} control opens hosts and returns from SSH config`, () => {
    const state = hooks();
    const { WorkspaceConnection } = load("components/WorkspaceConnection.tsx", {
      react: state.react,
      "../i18n": { ltr: (text) => text },
      "../paraglide/messages.js": messages,
      "./RemoteHostDialog": { RemoteHostDialog: "HostDialog" },
      "./RemoteStatus": { RemoteStatus: "RemoteStatus" },
      "./RemoteIcon": { RemoteIcon: "RemoteIcon" },
      "./SshConfigDialog": { SshConfigDialog: "ConfigDialog" },
      "./ui": { Button: "button", IconButton: "button" },
    });
    const runtime = { kind: "local", version: "test" };
    const render = () => { state.reset(); return nodes(WorkspaceConnection({ runtime, corner })); };
    assert.equal(render().some((node) => node.type === "HostDialog"), false);
    render().find((node) => node.type === "button" && node.props["aria-haspopup"] === "dialog").props.onClick();
    render().find((node) => node.type === "HostDialog").props.onConfigureSsh();
    assert.equal(render().some((node) => node.type === "HostDialog"), false);
    render().find((node) => node.type === "ConfigDialog").props.onClose();
    render().find((node) => node.type === "HostDialog").props.onClose();
    assert.equal(render().some((node) => node.type === "HostDialog" || node.type === "ConfigDialog"), false);

    state.reset();
    const remote = { kind: "ssh", session: { host: "research" } };
    const status = WorkspaceConnection({ runtime: remote, corner });
    assert.equal(status.type, "RemoteStatus");
    assert.equal(status.props.runtime, remote);
    assert.equal(status.props.corner, corner);
  });
}

for (const kind of ["local", "ssh"]) {
  for (const scenario of ["loading", "error", "onboarding", "projects"]) {
    test(`${kind} project home connection control during ${scenario}`, () => {
      const runtime = { kind, version: "test" };
      const { ProjectsPage } = load("routePages.tsx", {
        react: hooks().react,
        "./queries/client": {},
        "@tanstack/react-query": { useQuery: ({ kind }) => ({
          data: scenario === "loading" || scenario === "error" ? undefined
            : kind === "projects" ? [] : { onboardingCompleted: scenario !== "onboarding" },
          error: scenario === "error" ? new Error("offline") : null,
        }) },
        "./queries/projects": { listProjectsQuery: () => ({ kind: "projects" }), getUiStateQuery: () => ({ kind: "state" }) },
        "@tanstack/react-router": { useNavigate: () => () => {} },
        "./RemoteRuntime": { useRuntime: () => runtime },
        "./demoSessionState": {}, "./routeResume": {}, "./workspacePersistence": {}, "./panelLayout": {},
        "./paraglide/messages.js": messages,
        "./components/Onboarding": { Onboarding: "Onboarding" },
        "./components/ProjectsHome": { ProjectsHome: "ProjectsHome" },
        "./components/OfflineBanner": { OfflineBanner: "OfflineBanner" },
        "./components/WorkspaceConnection": { WorkspaceConnection: "WorkspaceConnection" },
        "./components/UpdateBanner": { UpdateBanner: "UpdateBanner", useUpdateStatus: () => ({}) },
        "./components/ui": {},
      });
      const connection = nodes(ProjectsPage()).find((node) => node.type === "WorkspaceConnection");
      if (kind === "local" && scenario !== "projects") {
        assert.equal(connection, undefined);
        return;
      }
      assert.ok(connection);
      assert.equal(connection.props.runtime, runtime);
      assert.equal(connection.props.corner, true);
    });
  }
}

for (const outcome of ["success", "failure", "popup-blocked"]) {
  test(`SSH host selection from home preserves new-tab behavior: ${outcome}`, async () => {
    const state = hooks();
    const calls = [];
    const { RemoteHostDialog } = load("components/RemoteHostDialog.tsx", {
      react: state.react,
      "react-dom": { createPortal: (tree) => tree },
      "@tanstack/react-query": {
        useQuery: ({ kind }) => ({ data: kind === "hosts" ? { hosts: [{ host: "research" }], defaultHost: null } : [] }),
        useMutation: () => ({ mutateAsync: async (args) => {
          calls.push(["connect", ...args]);
          if (outcome === "failure") throw new Error("connection failed");
          return { gatewayUrl: "http://localhost:4910/" };
        } }),
      },
      "lucide-react": { SlidersHorizontal: "Icon", X: "Icon" },
      "../api": {}, "../paraglide/messages.js": messages,
      "../paraglide/runtime.js": { getLocale: () => "en" },
      "../queries/settings": { getSshSettingsQuery: () => ({ kind: "hosts" }), listRemoteSessionsQuery: () => ({ kind: "sessions" }) },
      "../theme": { getThemePreference: () => "dark" },
      "./useDialogFocus": { useDialogFocus: () => {} },
      "./ui": {
        Button: "button", IconButton: "button", Input: "input", Spinner: "Spinner",
        showAlert: (...args) => calls.push(["alert", ...args]),
      },
    }, {
      document: { body: {} },
      window: { open: (...args) => {
        calls.push(["open", ...args]);
        return outcome === "popup-blocked" ? null : {
          location: { replace: (url) => calls.push(["navigate", url]) },
          close: () => calls.push(["close-tab"]),
        };
      } },
    });
    const tree = RemoteHostDialog({ onClose: () => calls.push(["close-dialog"]), onConfigureSsh: () => {} });
    nodes(tree).find((node) => node.type === "button" && nodes(node).some((child) => child.props.children === "research")).props.onClick();
    await new Promise(setImmediate);
    assert.deepEqual(calls[0], ["open", "/remote-launch", "_blank"]);
    if (outcome === "popup-blocked") {
      assert.deepEqual(calls.slice(1), [["alert", "remote_popup_blocked", "error"]]);
    } else {
      assert.deepEqual(calls[1], ["connect", "research", { theme: "dark", locale: "en" }]);
      assert.deepEqual(calls.slice(2), outcome === "success"
        ? [["navigate", "http://localhost:4910/"], ["close-dialog"]]
        : [["close-tab"], ["alert", "connection failed", "error"]]);
    }
  });
}
