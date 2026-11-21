import assert from "node:assert/strict";
import test from "node:test";
import { CITED_ID, firstLine, refLabel } from "../src/agentMessages.ts";
import { mentionsAsText } from "../src/components/Md.tsx";

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

test("a preview reads cited runs and files as words, never as their tags", () => {
  assert.equal(firstLine(mentionsAsText('Reached 1.80 bpc <run id="r1" />. The MLP <run id="r2" label="1.77 bpc"/> wins.')), "Reached 1.80 bpc. The MLP 1.77 bpc wins.");
  assert.equal(mentionsAsText('<run id="r1"/> See <file path="src/train.py" lines="3"/>'), "See src/train.py");
  assert.equal(mentionsAsText("Plain <b>text</b>."), "Plain <b>text</b>.");
});
