import { useMutation, useQuery } from "@tanstack/react-query";
import { FolderOpen } from "lucide-react";
import { useId, useState } from "react";

import type { OpenedSharedProject, SharedSource } from "../api";
import { m } from "../paraglide/messages.js";
import { slugify } from "./NewProjectForm";
import { useWorkspace } from "../workspace";
import { Button, Input, LoadingRow, Spinner, showAlert } from "./ui";

interface FormProps {
  source: SharedSource;
  onCancel: () => void;
  onOpened: (opened: OpenedSharedProject) => void;
}

/** Choose where a shared project goes, then open it; a published link shows its name first. */
export function OpenSharedProjectForm(props: FormProps) {
  const { source, onCancel } = props;
  const { api: { getPublicProject }, workspaceKey } = useWorkspace();
  const link = typeof source === "string" || source instanceof File ? null : source.link;
  const preview = useQuery({
    queryKey: workspaceKey("getPublicProject", link),
    queryFn: ({ signal }) => getPublicProject(link ?? "", signal),
    enabled: link !== null,
    retry: false,
  });
  if (typeof source === "string") return <PickFolder {...props} name={source.split(/[\\/]/).pop() ?? source} />;
  if (source instanceof File) return <PickFolder {...props} name={source.name} />;
  if (preview.error) return <div>
    <p role="alert" className="text-sm text-accent-red">{preview.error.message}</p>
    <div className="mt-6 flex justify-end"><Button type="button" variant="ghost" onClick={onCancel}>{m.projects_home_cancel()}</Button></div>
  </div>;
  const shared = preview.data?.sharedProject;
  return shared ? <PickFolder {...props} name={shared.name} intro={m.open_shared_link_body({ name: shared.name })} publisher={shared.publisher} /> : <LoadingRow />;
}

function PickFolder({ source, onCancel, onOpened, name, intro = m.open_shared_body({ file: name }), publisher }: FormProps & {
  name: string;
  intro?: string;
  publisher?: string | null;
}) {
  const { api: { importSharedProject }, queries: { getProjectPathStatusQuery } } = useWorkspace();
  const pathId = useId();
  const problemId = useId();
  const [path, setPath] = useState(`~/OpenResearch/${slugify(name.replace(/\.orx$/i, ""), 48)}`);
  const trimmed = path.trim();
  const status = useQuery({ ...getProjectPathStatusQuery(trimmed), enabled: Boolean(trimmed) }).data;
  const occupied = status?.exists === true && (status.directory === false || status.empty === false);
  const open = useMutation({ mutationFn: () => importSharedProject(trimmed, source), onSuccess: onOpened });
  const pending = open.isPending;

  return <form onSubmit={(event) => { event.preventDefault(); if (trimmed && !occupied && !pending) open.mutate(); }}>
    <p className="text-sm">{intro}</p>
    {publisher && <p className="mt-1 text-xs text-subtext">{m.open_shared_link_publisher({ name: publisher })}</p>}
    <label htmlFor={pathId} className="mt-5 mb-1 block text-sm font-medium">{m.open_shared_folder()}</label>
    <div className="relative">
      <FolderOpen size={16} className="pointer-events-none absolute start-3 top-3 text-subtext" />
      <Input id={pathId} autoFocus value={path} onChange={(event) => setPath(event.target.value)} disabled={pending} required
        aria-invalid={occupied} aria-describedby={occupied ? problemId : undefined} className="h-10 ps-10" />
    </div>
    {status?.resolvedPath && <p className="mt-1 break-all text-xs text-subtext">{status.resolvedPath}</p>}
    {occupied && <p id={problemId} className="mt-2 text-sm text-accent-red">{m.open_shared_folder_occupied()}</p>}
    {open.error && <p role="alert" className="mt-3 whitespace-pre-line text-sm text-accent-red">{open.error.message}</p>}
    <div className="mt-6 flex justify-end gap-2">
      <Button type="button" variant="ghost" disabled={pending} onClick={onCancel}>{m.projects_home_cancel()}</Button>
      <Button type="submit" variant="primary" disabled={pending || !trimmed || occupied}>{pending && <Spinner />}{m.open_shared_submit()}</Button>
    </div>
  </form>;
}

/** Tell the user about experiments that arrived without their code. */
export function reportMissingBranches({ missingBranches }: OpenedSharedProject) {
  if (missingBranches.length > 0) showAlert(m.open_shared_missing({ branches: missingBranches.join(", ") }), "warning");
}
