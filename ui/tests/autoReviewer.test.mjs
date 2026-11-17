import assert from "node:assert/strict";
import test from "node:test";
import { withAutoReviewer } from "../src/modelSelection.ts";
import { m } from "../src/paraglide/messages.js";
import { model } from "./models.mjs";

const choices = [
  { id: "manual", label: "Manual", description: "Always ask before making changes" },
  { id: "auto", label: "Auto", description: "from the server" },
];
const opencode = { id: "opencode", models: [{ ...model("cheap/mini"), displayName: "Mini Reviewer" }] };

test("OpenCode's Auto says which model reviews, and only Auto changes", () => {
  const same = withAutoReviewer(choices, opencode, null, "cloud/chat");
  assert.equal(same[1].description, m.opencode_auto_same_model());
  assert.equal(same[0], choices[0]);
  const picked = withAutoReviewer(choices, opencode, "cheap/mini", "cloud/chat");
  assert.equal(picked[1].description, m.opencode_auto_model({ model: "Mini Reviewer" }));
  const unlisted = withAutoReviewer(choices, opencode, "gone/deepseek-v4-flash", undefined);
  assert.equal(unlisted[1].description, m.opencode_auto_model({ model: "DeepSeek V4 Flash" }));
});

test("other agents keep their own wording", () => {
  assert.equal(withAutoReviewer(choices, { id: "claude-code", models: [] }, "cheap/mini", "x/y"), choices);
  assert.equal(withAutoReviewer(choices, undefined, null, null), choices);
});

test("a chat on a local model reviews with its own, whatever Settings names", () => {
  const local = withAutoReviewer(choices, opencode, "cheap/mini", "orx-local-1/qwen");
  assert.equal(local[1].description, m.opencode_auto_same_model());
});
