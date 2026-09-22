import assert from "node:assert/strict";
import test from "node:test";
import {
  initialPhase,
  recheckedAction,
  setupAction,
  terminalMounted,
} from "../src/components/harnessSetupState.ts";

const harness = (over) => ({ installed: true, installBroken: false, authState: "needsLogin", needsConfigRepair: false, ...over });

test("a repair state never starts a command on its own", () => {
  const repair = harness({ authState: "unsupported", needsConfigRepair: true });
  // No update command repairs a database the CLI will not open.
  const action = setupAction(repair);
  assert.equal(action, "login");
  const phase = initialPhase(repair, action);
  assert.equal(phase, "error");
  // The socket opens on mount for every phase but `preview`, so `error` alone
  // does not stop it — the terminal must not be mounted at all.
  assert.equal(terminalMounted(phase, false), false);
});

test("the terminal connects only after an approval, and outlives the run", () => {
  const login = harness();
  // Login is self-approving: the dialog opens straight into `running`.
  assert.equal(initialPhase(login, setupAction(login)), "running");
  assert.equal(terminalMounted("running", true), true);
  // Install waits for the Approve button; `preview` only echoes the command.
  const install = harness({ installed: false });
  assert.equal(setupAction(install), "install");
  assert.equal(initialPhase(install, "install"), "preview");
  assert.equal(terminalMounted("preview", false), true);
  // A run that finished keeps its output, including when the re-check that
  // followed it diagnosed a repair.
  assert.equal(terminalMounted("error", true), true);
  assert.equal(terminalMounted("success", true), true);
});

test("a cleared repair re-targets Retry; an uncleared one never does", () => {
  assert.equal(recheckedAction(harness({ authState: "unsupported" }), "login"), "update");
  assert.equal(recheckedAction(harness({ installed: false }), "login"), "install");
  assert.equal(recheckedAction(harness({ authState: "unsupported", needsConfigRepair: true }), "login"), null);
  assert.equal(recheckedAction(harness(), "login"), null);
  assert.equal(recheckedAction(undefined, "login"), null);
  // Re-targeting drops the old approval, so the new action stands down until
  // Retry presses it.
  assert.equal(terminalMounted("error", false), false);
});
