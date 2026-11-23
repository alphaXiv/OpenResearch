#!/usr/bin/env node
// Writes an HTML explorer of everything orx gives agents: MCP tools, the CLI,
// prompts and skills. `node scripts/agent-surface.mjs [out.html]`.

import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const target = process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(root, "target");
const out = resolve(process.argv[2] ?? join(target, "agent-surface.html"));
const dump = join(target, "agent-surface.json");

function cargo(args, env = {}) {
  execFileSync("cargo", args, { cwd: root, stdio: "inherit", env: { ...process.env, ...env } });
}

/** Lists alphaXiv's tools through `orx alphaxiv-mcp`, filtered by this computer's sign-in as agents see them. */
function listAlphaxiv(orx) {
  return new Promise((done) => {
    const child = spawn(orx, ["alphaxiv-mcp"], { stdio: ["pipe", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    const finish = (result) => {
      clearTimeout(timer);
      child.kill();
      done(result);
    };
    const timer = setTimeout(() => finish({ error: `orx alphaxiv-mcp did not list its tools in 20 s. ${stderr}`.trim() }), 20_000);
    child.on("error", (error) => finish({ error: `Could not start ${orx}: ${error.message}` }));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
      const lines = stdout.split("\n");
      stdout = lines.pop();
      const reply = lines.map((line) => JSON.parse(line)).find((message) => message.id === 1);
      if (reply) finish(reply.error ? { error: reply.error.message } : { tools: reply.result.tools });
    });
    child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list", params: {} })}\n`);
  });
}

function revision() {
  const attempts = [
    ["jj", ["log", "-r", "@", "--no-graph", "-T", 'change_id.shortest(8) ++ " " ++ description.first_line()']],
    ["git", ["log", "-1", "--format=%h %s"]],
  ];
  for (const [command, args] of attempts) {
    try {
      return execFileSync(command, args, { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
    } catch {}
  }
  return null;
}

mkdirSync(target, { recursive: true });
cargo(["test", "--locked", "--bin", "orx", "write_agent_surface", "--", "--ignored", "--exact", "local::agent_text::write_agent_surface"], {
  ORX_AGENT_SURFACE: dump,
});
cargo(["build", "--locked", "--bin", "orx"]);

const surface = JSON.parse(readFileSync(dump, "utf8"));
const orx = join(target, "debug", process.platform === "win32" ? "orx.exe" : "orx");
surface.mcp.alphaxivListing = await listAlphaxiv(orx);
surface.generatedAt = new Date().toISOString();
surface.revision = revision();

const page = readFileSync(join(root, "scripts", "agent-surface.html"), "utf8").replace(
  "<!--SURFACE-->",
  () => `<script type="application/json" id="surface">${JSON.stringify(surface).replaceAll("<", "\\u003c")}</script>`,
);
writeFileSync(out, page);
console.log(out);
