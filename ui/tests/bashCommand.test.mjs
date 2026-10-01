import assert from "node:assert/strict";
import test from "node:test";
import { bashCommand, withoutBashPrefix } from "../src/bashCommand.ts";

test("a leading ! turns the draft into a shell command", () => {
  assert.equal(bashCommand("!ls -la"), "ls -la");
  assert.equal(bashCommand("! git status "), "git status");
  assert.equal(bashCommand("!"), "");
});

test("only the first character counts", () => {
  assert.equal(bashCommand("ls !"), null);
  assert.equal(bashCommand(" !ls"), null);
  assert.equal(bashCommand(""), null);
});

test("leaving the mode keeps whatever was typed after the prefix", () => {
  assert.equal(withoutBashPrefix("!ls"), "ls");
  assert.equal(withoutBashPrefix("plain"), "plain");
});
