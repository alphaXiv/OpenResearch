import type { LucideIcon } from "lucide-react";

export function WorkspaceEmptyState({ icon: Icon, title, description }: {
  icon: LucideIcon;
  title: string;
  description?: string;
}) {
  return (
    <div className="flex h-full w-full min-w-0 flex-1 flex-col items-center justify-center gap-3 bg-background p-8 text-center text-subtext">
      <div className="flex size-16 shrink-0 items-center justify-center rounded-2xl border border-border-variant bg-surface"><Icon size={28} strokeWidth={1.5} aria-hidden="true" /></div>
      <h3 className="mt-1.5 mb-0 max-w-full wrap-anywhere text-xl font-medium text-text">{title}</h3>
      {description && <p className="m-0 w-full max-w-105 wrap-anywhere text-sm leading-relaxed text-subtext">{description}</p>}
    </div>
  );
}
