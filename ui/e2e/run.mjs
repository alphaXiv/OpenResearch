import { execFileSync, spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const worktree = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const results = join(worktree, "ui/e2e-results");
rmSync(results, { recursive: true, force: true });
mkdirSync(results, { recursive: true });
const fixture = mkdtempSync(join(tmpdir(), "orx-e2e-"));
const home = join(fixture, "home");
mkdirSync(join(home, ".codex"), { recursive: true });
mkdirSync(join(fixture, "bin"));
cpSync(join(worktree, "ui/e2e/codex.mjs"), join(fixture, "bin/codex"));
execFileSync("chmod", ["+x", join(fixture, "bin/codex")]);
writeFileSync(join(home, ".codex/auth.json"), JSON.stringify({ OPENAI_API_KEY: "e2e-not-a-real-key" }));
writeFileSync(join(fixture, "models.json"), JSON.stringify([{ id: "fixture-model", model: "fixture-model", displayName: "Fixture model", defaultReasoningEffort: "medium", serviceTiers: [{ id: "priority", name: "Fast", description: "Fixture tier" }], supportedReasoningEfforts: [{ reasoningEffort: "medium" }, { reasoningEffort: "high" }] }]));
writeFileSync(join(fixture, "native-requests.jsonl"), "");
// Reuse compiled dependencies, while the dev-slot helper still owns all ports/data/processes.
const cache = process.env.CARGO_TARGET_DIR ?? join(homedir(), ".local/share/openresearch-dev/cargo-target");
if (existsSync(cache)) {
  mkdirSync(join(home, ".local/share/openresearch-dev"), { recursive: true });
  symlinkSync(cache, join(home, ".local/share/openresearch-dev/cargo-target"));
}
const env = {
  PATH: `${join(fixture, "bin")}:${process.env.PATH}`,
  HOME: home,
  CARGO_HOME: process.env.CARGO_HOME ?? join(homedir(), ".cargo"),
  RUSTUP_HOME: process.env.RUSTUP_HOME ?? join(homedir(), ".rustup"),
  CARGO_INCREMENTAL: "0",
  ORX_NO_UPDATE_CHECK: "1",
  ORX_TELEMETRY_ENV: "off",
  ORX_E2E_FIXTURE: fixture,
  PLAYWRIGHT_BROWSERS_PATH: process.env.PLAYWRIGHT_BROWSERS_PATH ?? join(process.platform === "darwin" ? join(homedir(), "Library/Caches") : process.env.XDG_CACHE_HOME ?? join(homedir(), ".cache"), "ms-playwright"),
  ...(process.env.CI ? { CI: process.env.CI } : {}),
};
const helper = join(worktree, "scripts/dev-slot.mjs");
const slot = (command) => execFileSync(process.execPath, [helper, command, "--worktree", worktree, ...(command === "start" ? ["--db", "empty"] : [])], { env, encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] });
let state;
let dataDir;
try {
  console.log("Starting a fresh isolated E2E dev slot…");
  const output = slot("start");
  console.log(output);
  const url = output.match(/UI:\s+(http:\/\/\S+)/)?.[1];
  const statePath = output.match(/State:\s+(\S+)/)?.[1];
  if (!url || !statePath) throw new Error("Dev-slot helper did not report its URL/state");
  state = JSON.parse(readFileSync(statePath, "utf8"));
  dataDir = join(home, ".local/share/openresearch-dev", state.slotKey);
  writeFileSync(join(results, "run.json"), JSON.stringify({ revision: execFileSync("git", ["rev-parse", "HEAD"], { cwd: worktree, encoding: "utf8" }).trim(), dirty: execFileSync("git", ["status", "--porcelain"], { cwd: worktree, encoding: "utf8" }).trim().length > 0, command: ["pnpm -C ui test:e2e", ...process.argv.slice(2)].join(" "), url }, null, 2));
  const run = spawnSync(join(worktree, "ui/node_modules/.bin/playwright"), ["test", ...process.argv.slice(2)], {
    cwd: join(worktree, "ui"),
    stdio: "inherit",
    env: { ...env, ORX_E2E_URL: url, ORX_E2E_DATA: dataDir },
  });
  if (run.error) throw run.error;
  process.exitCode = run.status ?? 1;
} finally {
  const errors = [];
  try { console.log(slot("stop")); } catch (error) { errors.push(error); }
  try {
    if (state) {
      cpSync(dataDir, join(results, "data"), { recursive: true });
      cpSync(state.backendLog, join(results, "backend.log"));
      cpSync(state.uiLog, join(results, "ui.log"));
    }
    cpSync(join(fixture, "native-requests.jsonl"), join(results, "native-requests.jsonl"));
  } catch (error) { errors.push(error); }
  if (errors.length) {
    console.error(`Fixture preserved for diagnosis: ${fixture}`);
    throw new AggregateError(errors, "E2E shutdown or evidence collection failed");
  }
  try {
    console.log(slot("cleanup"));
    rmSync(fixture, { recursive: true, force: true });
  } catch (error) {
    console.error(`Fixture preserved for diagnosis: ${fixture}`);
    throw error;
  }
}
