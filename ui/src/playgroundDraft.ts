const KEY = "playground-draft-project";

export function getPlaygroundDraftProject(): string | null {
  try {
    return localStorage.getItem(KEY);
  } catch {
    return null;
  }
}

export function setPlaygroundDraftProject(projectId: string): void {
  try {
    localStorage.setItem(KEY, projectId);
  } catch {}
}

export function clearPlaygroundDraftProject(): void {
  try {
    localStorage.removeItem(KEY);
  } catch {}
}
