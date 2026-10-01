import type { ComputeTargetId } from "./api";
import { m } from "./paraglide/messages.js";

export const TARGET_LABELS: Record<ComputeTargetId, () => string> = {
  local: m.compute_target_local,
  tinker: m.compute_target_tinker,
  hf: m.compute_target_hf,
  modal: m.compute_target_modal,
  k8s: m.compute_target_k8s,
  ssh: m.compute_target_ssh,
  slurm: m.compute_target_slurm,
  ray: m.compute_target_ray,
  openresearch: m.compute_target_openresearch,
};
