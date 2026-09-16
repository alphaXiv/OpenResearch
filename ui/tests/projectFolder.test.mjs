import assert from "node:assert/strict";
import test from "node:test";
import { childProjectPath } from "../src/lib/projectFolder.ts";

test("new projects stay under the selected drive, POSIX root or UNC share", () => {
  assert.equal(childProjectPath("D:\\", "New Project"), "D:\\New Project");
  assert.equal(childProjectPath("D:/Research/", "实验"), "D:/Research\\实验");
  assert.equal(childProjectPath("\\\\server\\share\\", "new"), "\\\\server\\share\\new");
  assert.equal(childProjectPath("/", "new"), "/new");
  assert.equal(childProjectPath("~/OpenResearch", "new"), "~/OpenResearch/new");
  assert.equal(childProjectPath("/tmp/with\\backslash", "new"), "/tmp/with\\backslash/new");
});

test("a child name cannot become an absolute path, sibling or ancestor", () => {
  for (const name of ["", " ", ".", " .. ", "../sibling", "..\\sibling", "/tmp/new", "D:\\new", "a\0b", "a\nb"]) {
    assert.equal(childProjectPath("D:\\Research", name), null, JSON.stringify(name));
  }
});

test("Windows child names cannot be device aliases or alternate streams", () => {
  for (const name of ["NUL", "CON.txt", "com1", "LPT².txt", "project.", "name:stream", "a?b", "a*b", 'a"b', "a<b", "a|b"]) {
    assert.equal(childProjectPath("D:\\Research", name), null, name);
  }
  assert.equal(childProjectPath("D:\\Research", "console"), "D:\\Research\\console");
  assert.equal(childProjectPath("/tmp", "name:stream"), "/tmp/name:stream");
});
