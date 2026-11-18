import type { ChatSession, Project } from "./api";

export type SidebarRow =
  | { kind: "project"; project: Project; collapsed: boolean; busy: boolean }
  | { kind: "chat"; project: Project; session: ChatSession }
  | { kind: "section"; section: "working" | "done"; collapsed: boolean; count: number }
  | { kind: "inbox"; project: Project; session: ChatSession }
  | { kind: "done"; project: Project; session: ChatSession }
  | { kind: "spacer"; height: number }
  | { kind: "more"; project: Project }
  | { kind: "status"; project: Project; pending: boolean; error: boolean; retry: () => void; empty?: string };

/** Chats without a project share one group, whichever scratch project holds each. */
export const sidebarGroup = (project: Project) => project.scratch ? "scratch" : project.id;

/** How much of Done opening it shows at the rail's foot: five rows and half a sixth, the rest below the fold. */
export const DONE_PREVIEW_HEIGHT = 32 * 5.5;

export const sidebarRowHeight = (row: SidebarRow) =>
  row.kind === "spacer" ? row.height : row.kind === "project" ? 36 : row.kind === "inbox" ? 50
    : row.kind === "section" || row.kind === "done" ? 32 : 34;

export function fitSidebarRows(rows: SidebarRow[], height: number, activeId: string | null): SidebarRow[] {
  const selected = rows.find((row): row is Extract<SidebarRow, { kind: "chat" }> => row.kind === "chat" && row.session.id === activeId);
  const header = selected && rows.find((row) => row.kind === "project" && sidebarGroup(row.project) === sidebarGroup(selected.project));
  const fit = (candidates: SidebarRow[], available: number) => {
    const result: SidebarRow[] = [];
    let used = 0;
    for (const row of candidates) {
      if (used + sidebarRowHeight(row) > available) break;
      result.push(row);
      used += sidebarRowHeight(row);
    }
    const last = result.at(-1);
    if (last?.kind === "project" && !last.collapsed) result.pop();
    return result;
  };
  const visible = fit(rows, height);
  if (!selected || visible.includes(selected)) return visible;
  if (!header) return height < 34 ? visible : [...fit(rows.filter((row) => row !== selected), height - 34), selected];
  if (height < 70) return visible;
  return [
    ...fit(rows.filter((row) => !("project" in row) || sidebarGroup(row.project) !== sidebarGroup(selected.project)), height - 70),
    header,
    selected,
  ];
}
