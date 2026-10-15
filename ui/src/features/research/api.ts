import { get, post } from "../../api";
import type { RunMetrics } from "./comparison";

export interface DiagnosticReport {
  backend: string;
  ready: boolean;
  checkedAt: number;
  version: string;
  executable: string;
  buildChannel: string;
  checks: { id: string; status: "pass" | "fail"; message: string; action: string | null }[];
}

export const diagnoseCompute = (backend: string) =>
  post<DiagnosticReport>(`/api/settings/compute/doctor/${encodeURIComponent(backend)}`);
export const getProjectMetrics = (projectId: string, signal?: AbortSignal) =>
  get<Record<string, RunMetrics>>(`/api/projects/${encodeURIComponent(projectId)}/metrics`, signal);
export const importRunMetrics = (runId: string, metrics: unknown, replace: boolean) =>
  post<RunMetrics>(`/api/runs/${encodeURIComponent(runId)}/metrics`, { metrics, replace });
