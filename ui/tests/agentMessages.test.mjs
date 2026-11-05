import assert from "node:assert/strict";
import test from "node:test";
import { CITED_ID, firstLine, refLabel } from "../src/agentMessages.ts";

test("refs and chat ids are found whole, never in colours or longer ids", () => {
  const found = (text) => [...text.matchAll(CITED_ID)].map((match) => match[0]);
  assert.deepEqual(
    found("See msg_46de00b5, and `chat_7c35bf48-fd6e-4850-b485-c5cbc89a7134`."),
    ["msg_46de00b5", "chat_7c35bf48-fd6e-4850-b485-c5cbc89a7134"],
  );
  assert.deepEqual(found("#46de00b5 msg_46de00b5b msg_46de00b5-0000 my_msg_46de00b5 msg_1a3ed5db1001RW chat_root"), []);
  assert.equal(refLabel("msg_46de00b5"), "#46de00b5");
});

test("a preview is the first line without the markdown that leads or marks it", () => {
  assert.equal(firstLine("\n# **Outer** `Wilds`\n\nIt resets."), "Outer Wilds");
  assert.equal(firstLine("> - quoted"), "quoted");
  assert.equal(firstLine("Probably *Outer Wilds*, not snake_case_name."), "Probably Outer Wilds, not snake_case_name.");
});
