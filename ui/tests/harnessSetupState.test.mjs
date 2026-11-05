import assert from "node:assert/strict";
import test from "node:test";
import {
  initialPhase,
  recheckedAction,
  setupAction,
} from "../src/components/harnessSetupState.ts";

const harness = (over) => ({ installed: true, installBroken: false, authState: "needsLogin", needsConfigRepair: false, ...over });

test("a repair state never starts a command on its own", () => {
  const repair = harness({ authState: "unsupported", needsConfigRepair: true });
  // No update command repairs a database the CLI will not open.
  const action = setupAction(repair);
  assert.equal(action, "login");
  const phase = initialPhase(repair, action);
  assert.equal(phase, "error");
});

test("login starts immediately and installation asks first", () => {
  const login = harness();
  // Login is self-approving: the dialog opens straight into `running`.
  assert.equal(initialPhase(login, setupAction(login)), "running");
  const install = harness({ installed: false });
  assert.equal(setupAction(install), "install");
  assert.equal(initialPhase(install, "install"), "preview");
});

test("a cleared repair re-targets Retry; an uncleared one never does", () => {
  assert.equal(recheckedAction(harness({ authState: "unsupported" }), "login"), "update");
  assert.equal(recheckedAction(harness({ installed: false }), "login"), "install");
  assert.equal(recheckedAction(harness({ authState: "unsupported", needsConfigRepair: true }), "login"), null);
  assert.equal(recheckedAction(harness(), "login"), null);
  assert.equal(recheckedAction(undefined, "login"), null);
});
