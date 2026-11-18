---
name: orx-instances
description: "Create standalone OpenResearch compute instances with `orx instance create`. Use when the user wants a persistent machine for manual or ad-hoc work rather than an experiment run."
---

# Standalone instances

Provision a persistent instance in an organization when the user wants to SSH
in and work manually. Experiment runs use `orx exp run` instead.

```sh
orx instance create <orgId> --gpu H100_SXM --count 1 [--disk 100]
orx instance create <orgId> --gpu H100_SXM --provider runpod
orx instance create <orgId> --cpu cpu5g --vcpus 8
```

- `<orgId>` comes from `orx orgs`.
- Choose exactly one of `--gpu` or `--cpu`.
- `--count` and `--disk` apply to GPU instances; `--vcpus` applies to CPU.
- Omitting `--provider` chooses the cheapest matching offer; pass it to pin a
  provider.
- Provisioning is asynchronous and takes up to five minutes. Poll
  `orx instance list <orgId>` (or `--json`) for the box's status.
- A `failed` box shows its reason (`capacity_unavailable`, `readiness_timeout`,
  `provider_error`) and costs nothing; retry or pick another GPU from
  `orx compute`.
- Once `online`, the listing prints the `ssh` command to use. It pins the box's
  host key under the instance id; pass the same two `-o` options to `scp`
  (`-P` for the port) or `rsync -e`. Never pass `StrictHostKeyChecking=no`. A
  host-key warning means the box behind that id changed; stop and tell the user
  rather than bypassing it. `Permission denied (publickey)` means the box has no
  key this computer holds: run `orx ssh-key list`, then `orx ssh-key add`.
- Each box is tagged yours or another org member's (`createdByYou` in `--json`),
  with its live SSH sessions. Leave other members' boxes alone, and don't reuse
  one of yours that has sessions: an `orx exp run` box looks like any other.
- `orx` is not installed on the box. Run `orx` commands on this machine; to run
  an experiment on OpenResearch compute, use `orx exp run` rather than a
  standalone box.
- An OpenResearch-provisioned box bills from creation while `online` or
  `offline` until `orx instance delete <sandboxId>`; delete it when done.
