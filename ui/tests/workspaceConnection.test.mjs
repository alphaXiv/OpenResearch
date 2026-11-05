import { workspaceDom } from "./workspaceMocks.mjs";
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
    (name) => name === "../WorkspaceDom" ? { WorkspaceDomContext: {} } : name === "./EscapeScope" ? { EscapeScope: "escape-scope" } : mocks[name] ?? require(name), exports, ...Object.values(globals),
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
      useContext: () => workspaceDom,
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

const Menu = Object.fromEntries(["Root", "Trigger", "Portal", "Positioner", "Popup", "Item", "RadioGroup", "RadioItem", "RadioItemIndicator", "Separator"].map((name) => [name, `menu-${name}`]));

for (const corner of [true, false]) {
  test(`${corner ? "home" : "sidebar"} machine menu selects retained servers and configures the owning laptop`, () => {
    const state = hooks();
    const dialogs = [];
    const configurations = [];
    const selections = [];
    const owner = { queryClient: {} };
    const dialogController = { showCustomDialog: (options) => dialogs.push(options) };
    const session = { id: "server", host: "research", status: "connected" };
    const machines = { owner, activeId: "local", canSwitch: true, sessions: [session], selectLocal: () => selections.push("local"), selectRemote: (value) => selections.push(value) };
    const { WorkspaceConnection } = load("components/WorkspaceConnection.tsx", {
      react: state.react,
      "../workspace": { useWorkspace: () => ({ dialogs: dialogController, api: {} }) },
      "../machineSelection": { useMachines: () => machines },
      "@base-ui/react/menu": { Menu },
      "@base-ui/react/direction-provider": { DirectionProvider: "direction" },
      "../i18n": { ltr: String },
      "../paraglide/messages.js": messages,
      "./RemoteHostDialog": { RemoteHostDialog: "HostDialog" },
      "./RemoteStopDialog": { RemoteStopDialog: "StopDialog" },
      "./RemoteIcon": { RemoteIcon: "RemoteIcon" },
      "./SshConfigDialog": { showSshConfigDialog: (...args) => configurations.push(args) },
      "./Wordmark": { BrandMark: "BrandMark" },
      "./UpdateBanner": { useUpdateStatus: () => ({ status: null }) },
      "./ui": { Button: "button", MenuItem: "item", Tooltip: "tooltip" },
    });
    const render = () => { state.reset(); return nodes(WorkspaceConnection({ runtime: { kind: "local", version: "test" }, corner })); };
    const tree = render();
    assert.equal(tree.find((node) => node.type === Menu.Trigger).props["aria-label"], "remote_machine_switcher");
    assert.equal(tree.find((node) => node.type === Menu.RadioGroup).props.value, "local");
    tree.find((node) => node.type === Menu.RadioItem && node.props.value === "server").props.onClick();
    tree.find((node) => node.type === Menu.RadioItem && node.props.value === "local").props.onClick();
    assert.deepEqual(selections, [session, "local"]);
    tree.find((node) => node.type === Menu.Item && node.props.children === "remote_dialog_title").props.onClick();
    assert.equal(dialogs[0].component, "HostDialog");
    dialogs[0].props.onConfigureSsh();
    assert.equal(configurations[0][0], dialogController);
    assert.equal(configurations[0][2], owner);
    configurations[0][1]();
    assert.equal(dialogs.length, 2);
  });
}

for (const kind of ["local", "ssh"]) {
  for (const scenario of ["loading", "error", "onboarding", "no-projects", "projects"]) {
    test(`${kind} project home always offers machine selection during ${scenario}`, () => {
      const runtime = { kind, version: "test", ...(kind === "ssh" ? { session: { host: "research" } } : {}) };
      const { ProjectsPage } = load("routePages.tsx", {
        react: hooks().react,
        "./workspace": { useWorkspace: () => ({ queries: { listProjectsQuery: () => ({ kind: "projects" }), getUiStateQuery: () => ({ kind: "state" }) } }) },
        "@tanstack/react-query": { useQuery: ({ kind }) => ({
          data: scenario === "loading" || scenario === "error" ? undefined
            : kind === "projects" ? (scenario === "projects" ? [{ id: "p" }] : []) : { onboardingCompleted: scenario !== "onboarding" },
          error: scenario === "error" ? new Error("offline") : null,
        }) },
        "@tanstack/react-router": { useNavigate: () => () => {}, useRouteContext: () => ({ queryClient: {} }) },
        "./RemoteRuntime": { useRuntime: () => runtime },
        "./demoSessionState": {}, "./routeResume": {},
        "./paraglide/messages.js": messages,
        "./components/Onboarding": { Onboarding: "Onboarding" },
        "./components/ProjectsHome": { ProjectsEmptyState: "ProjectsEmptyState" },
        "./components/OfflineBanner": { OfflineBanner: "OfflineBanner" },
        "./components/WorkspaceConnection": { WorkspaceConnection: "WorkspaceConnection" },
        "./components/UpdateBanner": { UpdateBanner: "UpdateBanner", useUpdateStatus: () => ({}) },
        "./components/DesktopAppBanner": { DesktopAppBanner: "DesktopAppBanner" },
        "./components/ui": {},
      });
      const connection = nodes(ProjectsPage()).find((node) => node.type === "WorkspaceConnection");
      assert.equal(connection.props.runtime, runtime);
      assert.equal(connection.props.corner, true);
    });
  }
}

for (const outcome of ["success", "failure"]) {
  test(`SSH onboarding switches inside the app: ${outcome}`, async () => {
    const state = hooks();
    const calls = [];
    let finishConnection;
    const session = { id: "remote", host: "research", gatewayUrl: "http://localhost:4910/" };
    const owner = { queryClient: {}, queries: { getSshSettingsQuery: () => ({ kind: "hosts" }), listRemoteSessionsQuery: () => ({ kind: "sessions" }) }, api: { createRemoteSession: async (...args) => {
      calls.push(["connect", ...args]);
      await new Promise((resolve) => { finishConnection = resolve; });
      if (outcome === "failure") throw new Error("connection failed");
      return session;
    } } };
    const { RemoteHostDialog } = load("components/RemoteHostDialog.tsx", {
      react: state.react,
      "../machineSelection": { useMachines: () => ({ owner, selectRemote: (value) => calls.push(["select", value]) }) },
      "@tanstack/react-query": {
        useQuery: ({ kind }, client) => { assert.equal(client, owner.queryClient); return { data: kind === "hosts" ? { hosts: [{ host: "research" }], defaultHost: null } : [] }; },
        useMutation: ({ mutationFn }, client) => { assert.equal(client, owner.queryClient); return { mutateAsync: mutationFn }; },
      },
      "lucide-react": { SlidersHorizontal: "Icon", X: "Icon" },
      "../paraglide/messages.js": messages,
      "../i18n": { ltr: String },
      "./ProgressBar": { ProgressBar: "ProgressBar" },
      "../paraglide/runtime.js": { getLocale: () => "en" },
      "../theme": { getThemePreference: () => "dark" },
      "./ui": { Button: "button", IconButton: "button", Input: "input", Spinner: "Spinner" },
    }, { window: { open: () => { throw new Error("Unexpected browser popup"); } } });
    const props = {
      cancel: () => calls.push(["close-dialog"]),
      confirm: (openRemote) => Promise.resolve(openRemote()).then(() => calls.push(["close-dialog"]), (error) => calls.push(["error", error.message])),
      isDisabled: false, onConfigureSsh: () => {},
    };
    const render = () => { state.reset(); return RemoteHostDialog(props); };
    nodes(render()).find((node) => node.type === "button" && nodes(node).some((child) => child.props.children === "research")).props.onClick();
    assert.deepEqual(calls, [["connect", "research", { theme: "dark", locale: "en" }]]);
    props.isDisabled = true;
    const pending = nodes(render());
    assert.ok(pending.some((node) => node.props.role === "status" && nodes(node).some((child) => child.type === "ProgressBar")));
    assert.ok(pending.filter((node) => node.type === "button" || node.type === "input").every((node) => node.props.disabled));
    finishConnection();
    await new Promise(setImmediate);
    assert.deepEqual(calls.slice(1), outcome === "success" ? [["select", session], ["close-dialog"]] : [["error", "connection failed"]]);
    assert.ok(!nodes(render()).some((node) => node.type === "ProgressBar"));
  });
}

test("connection progress has a name and no invented percentage", () => {
  const { ProgressBar } = load("components/ProgressBar.tsx", {});
  const pending = ProgressBar({ label: "Connecting" });
  assert.equal(pending.props.role, "progressbar");
  assert.equal(pending.props["aria-label"], "Connecting");
  assert.equal(pending.props["aria-valuenow"], undefined);
  assert.ok(nodes(pending).some((node) => node.props.className?.includes("motion-safe:animate-")));

  const measured = ProgressBar({ value: 30, max: 50 });
  assert.equal(measured.props["aria-valuenow"], 60);
  assert.ok(nodes(measured).some((node) => node.props.style?.width === "60%"));
});
