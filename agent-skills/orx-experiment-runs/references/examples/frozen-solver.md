# Example: a frozen solver read by patterns

The solver binary is the harness: every experiment changes its input deck,
never its code. Its log reads `Step   120 of  2000  t=0.0600  residual=3.21e-05`
and ends with `METRIC l2_error 1.37e-04`.

```json
[{"regex": "^Reading mesh block (?P<step>\\d+)/(?P<of>\\d+)", "phase": "mesh", "unit": "blocks"},
 {"regex": "^Step\\s+(?P<step>\\d+) of\\s+(?P<of>\\d+) .*residual=(?P<residual>\\S+)", "phase": "solve", "unit": "steps"},
 {"regex": "^METRIC (?P<name>\\w+) (?P<value>\\S+)$"}]
```

```json
{"plan": [{"phase": "mesh"}, {"phase": "solve"}]}
```

```html
<signal name="l2_error" label="L2 error" digits="3" better="lower" />
<signal name="residual" label="Residual" digits="2" better="lower" />
<view headline="l2_error residual">
  <chart y="residual" y-scale="log" ghost="parent" />
  <stats of="l2_error residual" vs="final" />
</view>
```

```sh
orx exp view set <expId> --pattern patterns.json --config plan.json --file view.xml
```
