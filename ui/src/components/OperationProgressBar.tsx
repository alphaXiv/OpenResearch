import type { OperationProgress } from "../api";
import { fmtBytes } from "../i18n";
import { m } from "../paraglide/messages.js";
import { ProgressBar } from "./ProgressBar";

// Partial: a newer orx up may report a phase this page doesn't know.
const PHASE_LABELS: Partial<Record<string, () => string>> = {
  downloading: m.open_shared_downloading,
  unpacking: m.open_shared_unpacking,
  opening: m.open_shared_opening,
  preparing: m.share_publish_preparing,
  uploading: m.share_publish_uploading,
};

/** A background operation's phase, with its bytes when the phase counts them; mounted
 * before the first report so screen readers announce that one too. */
export function OperationProgressBar({ progress }: { progress: OperationProgress | null }) {
  if (!progress) return <span role="status" className="sr-only" />;
  const { phase, bytes, totalBytes } = progress;
  const label = PHASE_LABELS[phase]?.();
  const caption = totalBytes !== null
    ? m.operation_bytes({ done: fmtBytes(bytes), total: fmtBytes(totalBytes) })
    : bytes > 0 ? fmtBytes(bytes) : undefined;
  return <>
    <span role="status" className="sr-only">{label}</span>
    <ProgressBar label={label} value={totalBytes === null ? undefined : bytes} max={totalBytes ?? undefined} caption={caption} />
  </>;
}
