# cellstudio-cli-core

Shared building blocks for CellStudio tool CLIs (`segmenter`, `tracker`):

- `cli_core.registry`: algorithm registry (`Algorithm` spec: config model + lazily imported runner)
- `cli_core.config`: base pydantic config models and commented-YAML emit/load
- `cli_core.io`: image stack read/write helpers (tiff/npy)
- `cli_core.app`: the common `list` / `init` / `exec` typer app used by every tool
