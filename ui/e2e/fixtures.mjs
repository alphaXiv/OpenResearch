import { expect } from "@playwright/test";
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { randomUUID } from "node:crypto";

export const fixture = process.env.ORX_E2E_FIXTURE;
export const data = process.env.ORX_E2E_DATA;
export const sqlValue = (value) => `'${String(value).replaceAll("'", "''")}'`;
export const sql = (query) => execFileSync("sqlite3", [join(data, "orx.db")], { input: `.timeout 5000\n${query}\n`, encoding: "utf8" });
export const paneUrl = (project, pane, session = "new") => `/projects/${project.id}/tasks/${session}?${new URLSearchParams({ pane: JSON.stringify(pane) })}`;
export async function post(request, url, body) {
  const response = await request.post(url, { data: body });
  expect(response.ok(), await response.text()).toBeTruthy();
  return response.json();
}
export async function project(request) {
  const path = join(fixture, `project-${randomUUID()}`);
  const result = await post(request, "/api/projects", { name: "E2E research", path, createFolder: true, initializeGit: true, githubSyncEnabled: false });
  writeFileSync(join(path, "notes.txt"), "original notes\n");
  return result.project;
}
export async function session(request, project) {
  return (await post(request, "/api/chat/sessions", { projectId: project.id, harness: "codex", model: "fixture-model" })).session;
}
