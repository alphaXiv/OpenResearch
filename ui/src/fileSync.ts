export interface FileConflict {
  currentVersion: string | null;
  exists: boolean;
}

// Native dialogs can blur the editor before the user chooses whether to discard.
export let confirmingFileDiscard = false;

export function confirmFileDiscard(message: string): boolean {
  confirmingFileDiscard = true;
  try {
    return window.confirm(message);
  } finally {
    confirmingFileDiscard = false;
  }
}

export interface FileBufferState {
  path: string;
  draft: string;
  baseline: string;
  version: string;
  crlf: boolean;
  conflict: FileConflict | null;
}

export const normalizedFileContent = (content: string) => content.replace(/\r\n/g, "\n");

export const createFileBuffer = (path: string, content: string, version: string): FileBufferState => {
  const normalized = normalizedFileContent(content);
  return {
    path,
    draft: normalized,
    baseline: normalized,
    version,
    crlf: content.includes("\r\n"),
    conflict: null,
  };
};

export const isDirtyFileBuffer = (buffer: FileBufferState) =>
  buffer.draft !== buffer.baseline;

export const updateFileDraft = (buffer: FileBufferState, draft: string): FileBufferState => ({
  ...buffer,
  draft,
  conflict: draft === buffer.baseline ? null : buffer.conflict,
});

export const fileBufferContent = (buffer: FileBufferState) =>
  buffer.crlf ? buffer.draft.replace(/\n/g, "\r\n") : buffer.draft;

export function conflictAfterRefresh(
  buffer: FileBufferState,
  currentVersion: string | null,
  exists: boolean,
): FileConflict | null {
  if (!isDirtyFileBuffer(buffer) || (exists && currentVersion === buffer.version)) return null;
  return { currentVersion, exists };
}

// A tab owns its buffer and pending save even while its viewer is unmounted.
export class FileBufferSession {
  private buffer: FileBufferState | null = null;
  private listeners = new Set<() => void>();
  saving = false;
  saveError: string | null = null;
  private revision = 0;
  saveRevision = 0;

  getSnapshot = () => this.buffer;
  getRevision = () => this.revision;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };
  set = (buffer: FileBufferState | null) => {
    this.buffer = buffer;
    this.notify();
  };
  setSaving = (saving: boolean) => {
    this.saving = saving;
    if (saving) this.saveRevision++;
    this.notify();
  };
  saved = (draft: string, version: string) => {
    if (!this.buffer) return;
    this.set({ ...this.buffer, baseline: draft, version, conflict: null });
  };
  setSaveError = (error: string | null) => {
    this.saveError = error;
    this.notify();
  };
  private notify() {
    this.revision++;
    for (const listener of this.listeners) listener();
  }
  get needsProtection() {
    return this.saving || (this.buffer !== null && (isDirtyFileBuffer(this.buffer) || this.buffer.conflict !== null));
  }
}
