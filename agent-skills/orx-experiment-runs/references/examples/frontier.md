# Example: a speed and size frontier against a reference built in the run

A codec benchmark builds a reference and the candidate, then times both at
nine levels: its plan is `build`, then `bench` of 9 levels. Each level is one
row with both.

```python
for done, level in enumerate(levels, 1):
    ref, cand = bench(reference, level), bench(candidate, level)
    run.row("levels", level=level, ref_ms=ref.ms, ref_kb=ref.kb, cand_ms=cand.ms, cand_kb=cand.kb)
    run.step(done)
run.step(done, front_gain=gain, valid=int(bit_exact))
```

```html
<signal name="front_gain" label="Frontier gain" digits="4" better="higher" sign="true" />
<signal name="valid" label="Bit-exact" labels='{"1":"valid","0":"invalid"}' />
<signal name="cand_kb" label="Size" unit="KB" better="lower" />
<signal name="cand_ms" label="Compress time" unit="ms" better="lower" />
<view headline="front_gain">
  <chart from="levels" x-scale="log" ghost="parent" title="Size against compress time">
    <series x="ref_ms" y="ref_kb" label="Reference" ghost="false" />
    <series x="cand_ms" y="cand_kb" label="Candidate" />
  </chart>
  <stats of="front_gain valid" vs="final" />
  <table name="levels" columns="level cand_kb cand_ms ref_kb ref_ms" vs="final" />
</view>
```
