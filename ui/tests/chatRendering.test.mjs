import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("chat tool activity stays compact and omits redundant status rows", async () => {
  const source = await readFile(new URL("../src/components/ChatPanel.tsx", import.meta.url), "utf8");

  assert.match(source, /part\.tool\?\.toLowerCase\(\) === "interrupted"\) return false/);
  assert.match(source, /case "contextcompaction":[\s\S]*activity_compacting_context/);
  assert.match(source, /tailUnclassified[\s\S]*!rawPending\?\.progressLabel/);
  assert.match(source, /const TOOL_LINE_CLASS_NAME = "[^"]*line-clamp-2[^"]*break-words[^"]*"/);
  assert.equal(
    source.match(/\$\{TOOL_LINE_CLASS_NAME\}/g)?.length,
    2,
    "ToolRow and SubagentBlock must share the clamped line style",
  );
  assert.match(source, /busy && awaitingInput &&/);
});
