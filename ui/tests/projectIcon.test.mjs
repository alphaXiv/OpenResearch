import assert from "node:assert/strict";
import { test } from "node:test";
import { projectInitials } from "../src/components/ProjectIcon.tsx";

test("a project's initials take a letter from each of its first two words, else its first two letters", () => {
  assert.equal(projectInitials("Tokenizer ablations"), "TA");
  assert.equal(projectInitials("nanochat (demo)"), "ND");
  assert.equal(projectInitials("nanoGPT"), "NG");
  assert.equal(projectInitials("image_encoder-speedups"), "IE");
  assert.equal(projectInitials("orx"), "OR");
  assert.equal(projectInitials("模型实验"), "模型");
});
