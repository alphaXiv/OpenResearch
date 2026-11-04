import { Monitor } from "lucide-react";

export function RemoteIcon({ size = 16, className }: { size?: number; className?: string }) {
  return <Monitor size={size} className={className} aria-hidden="true" />;
}
