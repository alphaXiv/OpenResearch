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

export function createComposerState() {
  const composerSeeds = new Map<string, string>();
  return {
    composerStashes: new Map<string, ComposerStash>(),
    mountedComposers: new Map<string, (failed: ComposerStash) => void>(),
    seedComposer(projectId: string, prompt: string) {
      composerSeeds.set(projectId, prompt);
    },
    composerSeed(projectId: string) {
      return composerSeeds.get(projectId) ?? null;
    },
    forgetComposerSeed(projectId: string) {
      composerSeeds.delete(projectId);
    },
  };
}
