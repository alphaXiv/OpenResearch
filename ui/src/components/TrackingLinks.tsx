import { Copy, LineChart } from "lucide-react";
import { runTracking, type Run, type TrackingDescriptor } from "../api";
import { IconButton, IconButtonLink, showAlert } from "./ui";
import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";

function trackioUrl(record: Extract<TrackingDescriptor, { kind: "trackio" }>): string {
  const base = record.server_url.replace(/\/+$/, "");
  return record.project ? `${base}/?project=${encodeURIComponent(record.project)}` : `${base}/`;
}

function shellQuote(value: string): string {
  return `'${value.replaceAll("'", `'"'"'`)}'`;
}

function shellPath(path: string): string {
  return path.startsWith("~/") ? `"$HOME"/${shellQuote(path.slice(2))}` : shellQuote(path);
}

function tensorboardCommand(
  run: Run,
  record: Extract<TrackingDescriptor, { kind: "tensorboard" }>,
): string {
  const kind = typeof run.backend?.kind === "string" ? run.backend.kind : "";
  const host = typeof run.backend?.namespace === "string" ? run.backend.namespace : "";
  if ((kind === "slurm_job" || kind === "ssh_job") && host) {
    const command = `tensorboard --logdir ${shellPath(record.root_log_dir)} --host 127.0.0.1 --port 6006`;
    return `ssh -L 6006:localhost:6006 ${shellQuote(host)} ${shellQuote(command)}`;
  }
  return `tensorboard --logdir ${shellPath(record.root_log_dir)}`;
}

/** Copy a viewer command. When the clipboard refuses, the command stays on
 * screen as selectable text, so the button never fails silently. */
function copyCommand(command: string) {
  const showCommand = () =>
    showAlert(m.tracking_links_copy_tensorboard_failed(), "error", {
      description: (
        <code dir="ltr" className="select-all font-mono text-sm">
          {command}
        </code>
      ),
    });
  const clipboard = navigator.clipboard;
  if (!clipboard) {
    showCommand();
    return;
  }
  void clipboard
    .writeText(command)
    .then(() => showAlert(m.common_copied(), "success"))
    .catch(showCommand);
}

/** Tracking affordances derived only from this run's persisted launch record. */
export function TrackingLinks({ className, run }: { className?: string; run: Run }) {
  const records = runTracking(run);
  if (records.length === 0) return null;
  return (
    <>
      {records.map((record) =>
        record.kind === "trackio" ? (
          <IconButtonLink
            key={`trackio:${record.run}`}
            size="small"
            className={className}
            title={m.tracking_links_open_trackio_title({ run: ltr(record.run) })}
            aria-label={m.tracking_links_open_trackio({ run: ltr(record.run) })}
            href={trackioUrl(record)}
            target="_blank"
            rel="noopener noreferrer"
            onClick={(event) => event.stopPropagation()}
          >
            <LineChart size={13} />
          </IconButtonLink>
        ) : (
          <IconButton
            key={`tensorboard:${record.run}`}
            type="button"
            size="small"
            className={className}
            title={m.tracking_links_copy_tensorboard_title({ run: ltr(record.run) })}
            aria-label={m.tracking_links_copy_tensorboard({ run: ltr(record.run) })}
            onClick={(event) => {
              event.stopPropagation();
              copyCommand(tensorboardCommand(run, record));
            }}
          >
            <Copy size={13} />
          </IconButton>
        ),
      )}
    </>
  );
}
