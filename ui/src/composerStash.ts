import type { ChatTextAnnotation, TurnOptions } from "./api";

/** Per-chat stash for unsent composer content (draft, attachments, annotations). */
export interface ComposerAttachment {
  dataUrl: string;
  mediaType: string;
  name?: string;
  size: number;
}

export interface ComposerAnnotation extends ChatTextAnnotation {
  id: string;
  range?: Range;
}

export interface ComposerStash {
  draft: string;
  attachments: ComposerAttachment[];
  annotations: ComposerAnnotation[];
  unconfirmedSend: string | null;
  pendingClientTurn: { signature: string; id: string; mode: "steer" | null; options: TurnOptions; invalidated?: boolean } | null;
}

export const EMPTY_COMPOSER_STASH: ComposerStash = {
  draft: "",
  attachments: [],
  annotations: [],
  unconfirmedSend: null,
  pendingClientTurn: null,
};

/** Annotation Ranges reference transcript DOM that won't exist on return. */
export function composerStashContent(
  live: ComposerStash,
  prefillDraft: string | null,
): ComposerStash | null {
  const emptyDraft = live.draft === "" || live.draft === prefillDraft;
  if (emptyDraft && live.attachments.length === 0 && live.annotations.length === 0 && live.unconfirmedSend === null) {
    return null;
  }
  return {
    ...live,
    annotations: live.annotations.map(({ range: _range, ...annotation }) => annotation),
  };
}

const composerSeeds = new Map<string, string>();

/** A prompt for a new project's first chat, offered until the project has a chat. */
export function seedComposer(projectId: string, prompt: string): void {
  composerSeeds.set(projectId, prompt);
}

export function composerSeed(projectId: string): string | null {
  return composerSeeds.get(projectId) ?? null;
}

export function forgetComposerSeed(projectId: string): void {
  composerSeeds.delete(projectId);
}
