import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import * as react from "react";
import { renderToStaticMarkup } from "react-dom/server";
import * as jsx from "react/jsx-runtime";
import ts from "typescript";

// Run the real projects page with its surroundings stubbed; the decision stays real.
function loadProjectsPage(projects, state, calls = {}) {
  const output = ts.transpileModule(
    readFileSync(new URL("../src/routePages.tsx", import.meta.url), "utf8"),
    { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX } },
  ).outputText;
  const exports = {};
  const element = (marker) => () => react.createElement("div", { "data-marker": marker });
  new Function("require", "exports", output)((id) => {
    if (id === "react") return react;
    if (id === "react/jsx-runtime") return jsx;
    if (id === "@tanstack/react-query") {
      return {
        isCancelledError: () => false,
        useQuery: (options) => ({ data: options.queryKey[0] === "projects" ? projects : state, error: null }),
      };
    }
    if (id === "@tanstack/react-router") {
      return {
        useRouteContext: () => ({ queryClient: { setQueryData: (key, update) => { calls.cache = { key, value: update(projects) }; } } }),
        Link: element("link"),
        useNavigate: () => (to) => { calls.navigate = to; },
      };
    }
    if (id === "./queries/projects") {
      return {
        listProjectsQuery: () => ({ queryKey: ["projects"] }),
        getUiStateQuery: () => ({ queryKey: ["uiState"] }),
      };
    }
    if (id === "./RemoteRuntime") return { useRuntime: () => ({ kind: "local" }) };
    if (id === "./demoSessionState") return { clearReadDemoSessions: () => {} };
    if (id === "./routeResume") {
      return { globalResumeLocation: async () => "/projects", projectResumeLocation: async () => "/projects" };
    }
    if (id === "./paraglide/messages.js") {
      return { m: new Proxy({}, { get: (_, name) => () => String(name) }) };
    }
    if (id === "./components/Onboarding") return { Onboarding: element("onboarding") };
    if (id === "./components/ProjectsHome") {
      return { ProjectsEmptyState: (props) => { calls.emptyState = props; return element("no-projects")(); } };
    }
    if (id === "./components/OfflineBanner") return { OfflineBanner: () => null };
    if (id === "./components/WorkspaceConnection") return { WorkspaceConnection: () => null };
    if (id === "./components/UpdateBanner") return { UpdateBanner: () => null, useUpdateStatus: () => ({ status: null }) };
    if (id === "./components/DesktopAppBanner") return { DesktopAppBanner: () => null };
    if (id === "./components/ui") {
      return { Button: element("button"), Spinner: element("spinner"), showAlert: (message) => { calls.alert = message; } };
    }
    throw new Error(`Unexpected dependency: ${id}`);
  }, exports);
  return exports.ProjectsPage;
}

function render(projects, state, calls) {
  const ProjectsPage = loadProjectsPage(projects, state, calls);
  return renderToStaticMarkup(react.createElement(ProjectsPage));
}

test("a first run without projects opens the onboarding walkthrough", () => {
  const html = render([], { onboardingCompleted: false });
  assert.ok(html.includes('data-marker="onboarding"'), html);
});

test("deleting the last project lands on the empty state, not the walkthrough", () => {
  const html = render([], { onboardingCompleted: true });
  assert.ok(!html.includes('data-marker="onboarding"'), html);
  assert.ok(html.includes('data-marker="no-projects"'), html);
});

test("an existing project resumes instead of walking through onboarding again", () => {
  const html = render([{ id: "p" }], { onboardingCompleted: false });
  assert.ok(!html.includes('data-marker="onboarding"'), html);
  assert.ok(!html.includes('data-marker="no-projects"'), html);
});

test("creating a project from the empty state caches it and opens it", () => {
  const calls = {};
  render([], { onboardingCompleted: true }, calls);
  calls.emptyState.onCreated({ id: "new id" }, "publish failed");
  assert.deepEqual(calls.cache, { key: ["projects"], value: [{ id: "new id" }] });
  assert.deepEqual(calls.navigate, { href: "/projects/new%20id/settings/projects" });
  assert.equal(calls.alert, "publish failed");
});
