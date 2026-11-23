# Example: pretraining with an eval and SFT

The plan is known up front; `train`, `eval` and `sft` each report.

```json
{"plan": [{"phase": "download", "of": 9, "unit": "shards"},
          {"phase": "train", "of": 5000, "unit": "steps"},
          {"phase": "eval", "of": 22, "unit": "tasks"},
          {"phase": "sft", "of": 600, "unit": "steps"}]}
```

```html
<signal name="val_bpb" label="Val bpb" digits="3" better="lower" />
<signal name="loss" label="Train loss" digits="3" smooth="0.3" better="lower" />
<signal name="tok_per_sec" label="Tokens/s" as="compact" better="higher" />
<signal name="task" label="Task" />
<signal name="accuracy" label="Accuracy" digits="3" better="higher" />
<signal name="core" label="CORE" digits="3" better="higher" />
<signal name="sft_loss" label="SFT loss" digits="3" smooth="0.3" better="lower" />
<view headline="core val_bpb">
  <chart y="val_bpb" ghost="parent" phase="train" />
  <chart y="loss" phase="train" />
  <stats of="val_bpb loss tok_per_sec" phase="train" />
  <table name="tasks" columns="task accuracy" vs="final" phase="eval" />
  <chart y="sft_loss" ghost="parent" phase="sft" />
  <stats of="core val_bpb" vs="final" />
</view>
```
