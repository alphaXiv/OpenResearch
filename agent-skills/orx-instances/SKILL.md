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
- Provisioning is asynchronous. The command prints the instance id and status;
  its SSH host appears when the machine becomes ready.
