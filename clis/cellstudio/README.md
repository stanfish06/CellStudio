# cellstudio-cli

Umbrella CLI. Discovers tool CLIs under `clis/` (a tool = a directory whose `pyproject.toml` exposes a console script named after the directory) and dispatches into each tool's own uv environment.

```sh
# install the umbrella command once
uv tool install --editable clis/cellstudio

cellstudio tool list                          # -> segmenter, tracker
cellstudio run segmenter list                 # algorithms + versions
cellstudio run segmenter init --algo cellpose # emit config yaml
cellstudio run segmenter exec cellpose.yaml   # run it
```

`cellstudio run <tool> ...` is equivalent to `uv run --project clis/<tool> <tool> ...`.
If the CLI cannot locate `clis/` (non-editable install, running outside the repo), set `CELLSTUDIO_CLIS_DIR`.
