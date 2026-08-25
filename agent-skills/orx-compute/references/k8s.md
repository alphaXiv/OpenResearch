# Kubernetes (`--backend k8s`)

Use this backend only when the user explicitly requests Kubernetes or it is the
configured default. Authentication comes from the user's kubeconfig; the
context and namespace come from their configured Kubernetes profile.

There are no flavors. The run shape is a Kubernetes manifest committed on the
experiment branch, defaulting to `.orx/k8s.yaml` and overridable with
`--manifest <path>`. Inspect the cluster before choosing resources.

```sh
orx exp run <expId> --backend k8s
orx exp run <expId> --backend k8s --manifest infra/run.yaml --timeout 8h
```

A minimal manifest:

```yaml
apiVersion: batch/v1
kind: Job
metadata:
  name: train-{{ORX_RUN}}
spec:
  template:
    spec:
      restartPolicy: Never
      containers:
        - name: run
          image: pytorch/pytorch:2.6.0-cuda12.4-cudnn9-runtime
          command: ["bash", "-c", "$ORX_SCRIPT"]
          resources:
            requests: { nvidia.com/gpu: "4", cpu: "32", memory: "128Gi" }
            limits: { nvidia.com/gpu: "4" }
```

The submit-time contract is:

- Exactly one Job represents the primary outcome. With several Jobs, label the
  primary `orx-primary: "true"`. Parallel and Indexed Jobs are rejected because
  the immutable archive is staged into one pod.
- A container in the primary Job must execute `$ORX_SCRIPT`, normally with
  `command: ["bash", "-c", "$ORX_SCRIPT"]`.
- Every resource needs `metadata.name`; do not use `generateName` or set a
  foreign namespace. Include `{{ORX_RUN}}` in names to prevent rerun collisions.
- `orx` injects run labels and the `orx-env` Secret into primary containers. It
  also supplies defaults for `activeDeadlineSeconds`,
  `ttlSecondsAfterFinished`, and `backoffLimit: 0` when absent.
- Auxiliary resources may accompany the Job and are deleted on cancellation.
  They must reference the `orx-env` Secret themselves when needed.
- The run log follows the primary Job's sole pod.
- A detached `orx supervise` process watches the Job through `kubectl`; do not
  kill it.
