# Example: an eval by category

The run reports `run.row("categories", category=c, accuracy=acc)` as each
category finishes and `run.step(done, accuracy=overall)` per task.

```html
<signal name="category" label="Category" />
<signal name="accuracy" label="Accuracy" digits="3" better="higher" />
<view headline="accuracy">
  <chart from="categories" x="category" y="accuracy" mark="bars" ghost="parent" />
  <stats of="accuracy" />
  <table name="categories" columns="category accuracy" vs="final" />
</view>
```
