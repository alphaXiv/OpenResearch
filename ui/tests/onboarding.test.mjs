import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import * as react from "react";
import { renderToStaticMarkup } from "react-dom/server";
import * as jsx from "react/jsx-runtime";
import ts from "typescript";

// Run the real projects page with its surroundings stubbed; the decision stays real.
function loadProjectsPage(projects, state) {
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
      return { useRouteContext: () => ({ queryClient: {} }), Link: element("link"), useNavigate: () => () => {} };
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
    if (id === "./components/ProjectsHome") return { ProjectsEmptyState: element("no-projects") };
    if (id === "./components/OfflineBanner") return { OfflineBanner: () => null };
    if (id === "./components/WorkspaceConnection") return { WorkspaceConnection: () => null };
    if (id === "./components/UpdateBanner") return { UpdateBanner: () => null, useUpdateStatus: () => ({ status: null }) };
    if (id === "./components/DesktopAppBanner") return { DesktopAppBanner: () => null };
    if (id === "./components/ui") {
      return { Button: element("button"), Spinner: element("spinner"), showAlert: () => {} };
    }
    throw new Error(`Unexpected dependency: ${id}`);
  }, exports);
  return exports.ProjectsPage;
}

function render(projects, state) {
  const ProjectsPage = loadProjectsPage(projects, state);
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
