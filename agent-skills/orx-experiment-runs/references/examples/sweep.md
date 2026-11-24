# Example: a sweep inside one run

One run trains eight learning rates briefly, then refits the best. Each rate
is a step of `sweep` and a row; its curve is rows too, one series per rate.

```json
{"plan": [{"phase": "sweep", "of": 8, "unit": "rates"}, {"phase": "refit", "of": 2000, "unit": "steps"}]}
```

```python
best = float("inf")
for done, lr in enumerate(rates, 1):
    for step, loss in train(lr, steps=200):
        if step % 20 == 0:
            run.row("curves", step=step, lr=lr, loss=loss)
    val = evaluate()
    best = min(best, val)
    run.row("points", lr=lr, val_loss=val)
    run.step(done, best_val_loss=best)
with run.phase("refit", total=2000, unit="steps") as p:
    for step, loss in train(best_lr, steps=2000):
        p.step(step, loss=loss)
    p.step(step, val_loss=evaluate())
```

```html
<signal name="val_loss" label="Val loss" digits="3" better="lower" />
<signal name="best_val_loss" label="Best sweep val loss" digits="3" better="lower" />
<signal name="loss" label="Loss" digits="3" better="lower" />
<view headline="val_loss best_val_loss">
  <chart from="curves" x="step" y="loss" by="lr" phase="sweep" title="Loss by learning rate" />
  <chart from="points" x="lr" y="val_loss" x-scale="log" mark="points" ghost="parent" />
  <chart y="loss" phase="refit" ghost="parent" />
</view>
```
