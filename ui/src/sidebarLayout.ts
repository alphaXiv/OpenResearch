import type { ChatSession, Project } from "./api";

export type SidebarRow =
  | { kind: "project"; project: Project; collapsed: boolean; busy: boolean }
  | { kind: "chat"; project: Project; session: ChatSession }
  | { kind: "more"; project: Project }
  | { kind: "status"; project: Project; pending: boolean; error: boolean; retry: () => void };

export const sidebarRowHeight = (row: SidebarRow) => row.kind === "project" ? 36 : 34;

export function fitSidebarRows(rows: SidebarRow[], height: number, activeId: string | null): SidebarRow[] {
  const selected = rows.find((row) => row.kind === "chat" && row.session.id === activeId);
  const header = selected && rows.find((row) => row.kind === "project" && row.project.id === selected.project.id);
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
    ...fit(rows.filter((row) => row.project.id !== selected.project.id), height - 70),
    header,
    selected,
  ];
}
