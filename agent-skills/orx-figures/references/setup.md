# Set up once per project

Vendor the style module next to the figure scripts so a figure stays
reproducible after this session ends:

```sh
mkdir -p figs && orx skill figures/assets/orx_figstyle.py > figs/orx_figstyle.py
```

That works from any directory. Vendor it next to whichever destination the
script lives in: a report's script under the artifacts directory needs its own
copy there, or the import fails. Check the file is non-empty before importing
it, since the redirect creates the target before the lookup runs.

Load `orx-python-env` for the environment. The Python plotting templates
use `matplotlib` and `numpy`. For independent plots using uv:

```sh
uv run --no-project --with matplotlib --with numpy python figs/loss_curve.py
```

For saved scripts with inline dependency metadata, use
`uv run --no-project figs/loss_curve.py` so uv reads that metadata.
Plots importing project code must run in the project environment, not the
isolated environment above.
