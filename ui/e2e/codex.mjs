#!/usr/bin/env node
import { appendFileSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { randomUUID } from "node:crypto";

const root = process.env.ORX_E2E_FIXTURE;
const emit = (value) => process.stdout.write(`${JSON.stringify(value)}\n`);
if (process.argv.includes("--version")) {
  console.log("codex-cli 0.144.0");
} else if (process.argv.includes("exec")) {
  emit({ type: "item.completed", item: { type: "agent_message", text: "Fixture one-shot response" } });
} else if (process.argv.includes("app-server")) {
  const threads = new Map();
  for await (const line of createInterface({ input: process.stdin })) {
    const request = JSON.parse(line);
    if (request.id === undefined || !request.method) continue;
    appendFileSync(join(root, "native-requests.jsonl"), `${line}\n`);
    const params = request.params ?? {};
    let result;
    switch (request.method) {
      case "initialize": result = { userAgent: "orx-e2e" }; break;
      case "model/list": result = { data: JSON.parse(readFileSync(join(root, "models.json"), "utf8")) }; break;
      case "thread/start":
      case "thread/resume": {
        const id = params.threadId ?? randomUUID();
        threads.set(id, params.model ?? "fixture-default");
        result = { thread: { id }, model: threads.get(id) };
        break;
      }
      case "turn/start": {
        const turnId = randomUUID();
        const model = params.model ?? threads.get(params.threadId) ?? "fixture-default";
        emit({ id: request.id, result: { turn: { id: turnId, status: "inProgress" } } });
        const event = (method, values) => emit({ method, params: { threadId: params.threadId, turnId, ...values } });
        event("item/completed", { item: { id: randomUUID(), type: "agentMessage", text: `E2E reply using ${model}`, phase: "final_answer" } });
        event("turn/completed", { turn: { id: turnId, status: "completed" } });
        continue;
      }
      case "turn/interrupt": result = {}; break;
      default:
        emit({ id: request.id, error: { code: -32601, message: `Unimplemented fixture method: ${request.method}` } });
        continue;
    }
    emit({ id: request.id, result });
  }
} else {
  console.error("Unexpected fixture arguments", process.argv.slice(2));
  process.exitCode = 1;
}
