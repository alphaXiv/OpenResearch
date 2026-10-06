import type { Harness } from "../api";

export function claudeProviderLabel(h: Pick<Harness, "id" | "authProvider">): string | null {
  if (h.id !== "claude-code") return null;
  switch (h.authProvider) {
    case "bedrock": return "Amazon Bedrock";
    case "vertex": return "Google Vertex AI";
    case "foundry": return "Microsoft Foundry";
    case "firstParty": return null;
    default: return h.authProvider ?? null;
  }
}
