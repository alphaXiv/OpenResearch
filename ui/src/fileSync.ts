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
