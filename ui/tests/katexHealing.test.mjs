import assert from "node:assert/strict";
import test from "node:test";
import { healKatexSource } from "../src/katexHealing.ts";

test("valid math is left alone", () => {
  assert.equal(healKatexSource("a^2+b^2", false, "escape"), "a^2+b^2");
});

test("double-escaped commands are repaired", () => {
  assert.equal(healKatexSource("\\\\frac{a}{b}", false, "escape"), "\\frac{a}{b}");
  assert.equal(healKatexSource("\\\\alpha+1", false, "escape"), "\\alpha+1");
});

test("a streamed tail closes its braces or drops the dangling token", () => {
  assert.equal(healKatexSource("\\frac{a}{b", false, "trim", true), "\\frac{a}{b}");
  assert.equal(healKatexSource("x^{2} + \\", false, "trim", true), "x^{2} + ");
  assert.equal(healKatexSource("x_", false, "trim", true), "x");
});

test("unparseable text is trimmed while streaming and kept as text once finished", () => {
  assert.equal(healKatexSource("a + \\notamacro{b}", false, "trim"), "a + ");
  assert.equal(healKatexSource("a + \\notamacro{b}", false, "escape"), "a + \\text{\\textbackslash{}notamacro\\{b\\}}");
});
