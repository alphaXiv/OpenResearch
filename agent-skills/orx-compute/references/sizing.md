# Sizing compute

```sh
orx compute                            # browse GPU offers across all providers
orx compute --gpu H100_SXM --count 1   # filter GPU offers
orx compute --cpu                      # browse CPU-only offers
```

- Decide GPU versus CPU first. API-driven evaluation and data preparation often
  run more cheaply on CPU.
- Pick the smallest shape that fits the model and a minimal batch.
- Escalate after a real OOM or hopelessly slow run instead of starting with the
  largest accelerator.
- Raise the timeout only for genuinely long runs.
