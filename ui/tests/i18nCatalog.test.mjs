import assert from "node:assert/strict";
import { cp, mkdir, mkdtemp, readFile, rm, unlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

async function fixture() {
  const root = await mkdtemp(join(tmpdir(), "orx-i18n-"));
  await mkdir(join(root, "scripts"));
  await cp(new URL("../scripts/check-i18n.mjs", import.meta.url), join(root, "scripts/check-i18n.mjs"));
  await cp(new URL("../messages", import.meta.url), join(root, "messages"), { recursive: true });
  await cp(new URL("../project.inlang", import.meta.url), join(root, "project.inlang"), { recursive: true });
  return root;
}

const run = (root) => spawnSync(process.execPath, [join(root, "scripts/check-i18n.mjs")], { encoding: "utf8" });
const catalog = async (root, locale) => JSON.parse(await readFile(join(root, "messages", `${locale}.json`), "utf8"));
const save = (root, locale, value) => writeFile(join(root, "messages", `${locale}.json`), `${JSON.stringify(value, null, 2)}\n`);
const expectFailure = (root, expected, message) => {
  const result = run(root);
  assert.notEqual(result.status, 0, message);
  assert.match(result.stderr, expected, message);
};

test("catalog lint rejects missing files, key drift, and invalid values", async () => {
  const roots = [];
  try {
    const missingFile = await fixture(); roots.push(missingFile);
    await unlink(join(missingFile, "messages/fa.json"));
    expectFailure(missingFile, /fa: .*ENOENT/, "missing locale file");

    const missingKey = await fixture(); roots.push(missingKey);
    const zhMissing = await catalog(missingKey, "zh-CN");
    delete zhMissing.common_save;
    await save(missingKey, "zh-CN", zhMissing);
    expectFailure(missingKey, /zh-CN: missing keys: common_save/, "missing key");

    const unexpectedKey = await fixture(); roots.push(unexpectedKey);
    const faUnexpected = await catalog(unexpectedKey, "fa");
    faUnexpected.unexpected_test_key = "اضافی";
    await save(unexpectedKey, "fa", faUnexpected);
    expectFailure(unexpectedKey, /fa: unexpected keys: unexpected_test_key/, "unexpected key");

    for (const [name, value, expected] of [
      ["empty", "", /empty or non-string values: common_save/],
      ["non-string", 7, /empty or non-string values: common_save/],
    ]) {
      const root = await fixture(); roots.push(root);
      const fa = await catalog(root, "fa");
      fa.common_save = value;
      await save(root, "fa", fa);
      expectFailure(root, expected, name);
    }
  } finally {
    await Promise.all(roots.map((root) => rm(root, { recursive: true, force: true })));
  }
});
