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
