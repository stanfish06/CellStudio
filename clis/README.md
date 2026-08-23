# CellStudio CLIs

Standalone CLIs for automatic segmentation and tracking. 

```
clis/
  cellstudio/   umbrella CLI: `cellstudio tool list`, `cellstudio run <tool> ...`
  core/         cellstudio-cli-core: shared config/io/registry + the list/init/exec app
  segmenter/    cellpose, stardist, micro-sam  -> label masks
  tracker/      laptrack, ultrack              -> tracks + relabeled masks
```

## Usage

```sh
uv tool install clis/cellstudio   # once; puts `cellstudio` on PATH (-e for umbrella dev)

cellstudio tool list                          # segmenter, tracker
cellstudio run segmenter list                 # algorithms + installed versions
cellstudio run segmenter init --algo cellpose # emit cellpose.yaml (full option mapping)
cellstudio run segmenter exec cellpose.yaml   # run it (CPU by default)
cellstudio run segmenter exec cellpose.yaml --gpu   # use the GPU where the algorithm supports it

cellstudio run tracker init --algo laptrack
cellstudio run tracker exec laptrack.yaml
```

`cellstudio run <tool> ...` == `uv run --project clis/<tool> <tool> ...`

## Config yaml

Every algorithm config has the same shape; `init` emits it fully commented with API defaults:

```yaml
tool: segmenter
algorithm: cellpose
io:
  input:  { image: nuclei.tif }   # named inputs
  output: { masks: masks.tif }    # named outputs
options: ...                      # full mapping of the underlying API
```

Segmenter input: most microscopy formats via [bioio](https://github.com/bioio-devs/bioio), nd2 (Nikon), czi (Zeiss), lif (Leica), dv (DeltaVision), png/jpg, ome-zarr, npy, or a directory of images (stacked on axis 0). Non-tiff reads report dims, channel names, and physical pixel sizes; `io.input.scene`/`io.input.channel` select from multi-scene/multi-channel files. The Bio-Formats long tail (vsi, ims, oir, ...) is opt-in: `uv sync --group bioformats` in `clis/segmenter` (needs a Java runtime). Output: label image, 0 = background. Tracker input: TYX/TZYX label stack. Output: `tracks.csv` (+ splits/merges for laptrack), plus masks relabeled by track id.

First `exec` of cellpose / micro-sam / stardist downloads pretrained weights into the user cache. `exec` runs on CPU unless `--gpu` is passed (cellpose/micro-sam: torch device, stardist: tensorflow GPU visibility; laptrack and ultrack are CPU-only and say so). A `device` set in the config overrides the flag. Ultrack keeps its working database under `options.data.working_dir` (default `.ultrack/` in the cwd).

## CPU vs CUDA torch

Default installs get CPU torch wheels (`download.pytorch.org/whl/cpu`). Each torch-using tool has `cpu`/`gpu` dependency-groups (`default-groups = ["cpu"]`); the `gpu` group pins torch to the cu130 index for cloud GPU machines:

```sh
uv sync --no-group cpu --group gpu          # inside clis/<tool>, or --project clis/<tool>
```

`cellstudio run <tool> ... --gpu` does this automatically: when an NVIDIA device is present it dispatches with `--no-group cpu --group gpu`; otherwise it warns and keeps CPU torch. Standalone `uv run` users on GPU machines must pass the group flags themselves (plain `uv run` re-syncs back to the cpu group).

## Adding an algorithm

Create `src/<tool>/tools/<name>/` with `config.py` (pydantic models: IO paths + full options mapping), `io.py` (load input / save output), `runner.py` (`run(cfg) -> dict`, heavy imports inside), then register it in `src/<tool>/registry.py`. `list`/`init`/`exec` come from `cli_core.app`.

## Adding a tool

Copy the layout of `segmenter/`: a uv project whose `pyproject.toml` exposes a console script named after the directory, depending on `cellstudio-cli-core` (path source). `cellstudio tool list` discovers it automatically.
