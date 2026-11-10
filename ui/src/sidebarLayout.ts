import type { ChatSession, Project } from "./api";

export type SidebarRow =
  | { kind: "project"; project: Project; collapsed: boolean; busy: boolean }
  | { kind: "section"; section: "projects" | "playground" | "all"; open: boolean; count: number }
  | { kind: "chat"; project: Project; session: ChatSession }
  | { kind: "more"; project: Project }
  | { kind: "status"; project: Project; pending: boolean; error: boolean; retry: () => void };

export const isPlaygroundProject = (project: Project) => project.kind === "playground";

export const sidebarRowHeight = (row: SidebarRow) =>
  row.kind === "project" || row.kind === "section" ? 36 : 34;

export function fitSidebarRows(rows: SidebarRow[], height: number, activeId: string | null): SidebarRow[] {
  const selected = rows.find(
    (row): row is Extract<SidebarRow, { kind: "chat" }> =>
      row.kind === "chat" && row.session.id === activeId,
  );
  const selectedPlayground = selected ? isPlaygroundProject(selected.project) : false;
  const header = selected && (selectedPlayground
    ? rows.find((row) => row.kind === "section" && row.section === "playground")
    : rows.find((row) => row.kind === "project" && row.project.id === selected.project.id));
  const fit = (candidates: SidebarRow[], available: number) => {
    // Section headers are the rail's map: reserve their height up front so
    // collapsing/expanding content never pushes the Playground header out of
    // view in the compact rail.
    const sectionHeight = candidates.reduce(
      (height, row) => height + (row.kind === "section" ? sidebarRowHeight(row) : 0),
      0,
    );
    const budget = Math.max(0, available - sectionHeight);
    const result: SidebarRow[] = [];
    let used = 0;
    let truncated = false;
    for (const row of candidates) {
      if (row.kind === "section") {
        result.push(row);
        continue;
      }
      const height = sidebarRowHeight(row);
      if (truncated || used + height > budget) {
        truncated = true;
        continue;
      }
      result.push(row);
      used += height;
    }
    const last = result.at(-1);
    if (last?.kind === "project" && !last.collapsed) result.pop();
    return result;
  };
  const visible = fit(rows, height);
  if (!selected || visible.includes(selected)) return visible;
  if (!header) return height < 34 ? visible : [...fit(rows.filter((row) => row !== selected), height - 34), selected];
  if (height < 70) return visible;
  if (selectedPlayground) {
    return [
      ...fit(rows.filter((row) => row !== header && row !== selected), height - 70),
      header,
      selected,
    ];
  }
  return [
    ...fit(
      rows.filter(
        (row) => row.kind !== "section" && row.project.id !== selected.project.id,
      ),
      height - 70,
    ),
    header,
    selected,
  ];
}
