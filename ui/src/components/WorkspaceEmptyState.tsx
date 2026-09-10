import type { LucideIcon } from "lucide-react";

export function WorkspaceEmptyState({ icon: Icon, title, description }: {
  icon: LucideIcon;
  title: string;
  description?: string;
}) {
  return (
    <div className="flex h-full w-full min-w-0 flex-1 flex-col items-center justify-center gap-1.5 bg-background p-6 text-center text-muted">
      <Icon size={36} strokeWidth={1.5} className="shrink-0" />
      <h3 className="mt-1.5 mb-0 max-w-full wrap-anywhere text-xl font-medium text-text">{title}</h3>
      {description && <p className="m-0 w-full max-w-105 wrap-anywhere text-base leading-[1.55] text-subtext">{description}</p>}
    </div>
  );
}
