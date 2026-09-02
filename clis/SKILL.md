---
name: cellstudio-cli
description: Runs automatic cell segmentation (cellpose, stardist, micro-sam) and tracking (laptrack, ultrack) via the repo's CLIs. Use when asked to segment microscopy images, track cells/labels over time, generate or edit a segmenter/tracker config yaml, or add a new algorithm or tool under clis/.
---

# CellStudio CLIs

Config-driven wrappers around segmentation/tracking libraries. Every algorithm follows the same workflow: `init` emits a fully commented yaml (every option of the underlying API, at API defaults), you edit the `io:` paths (and options as needed), `exec` validates strictly, runs, and prints output paths plus object/track counts. All behavior switches live in the config, not CLI flags (e.g. laptrack `options.mode: overlap|centroid`). Do not hand-write configs from memory: unknown keys are rejected; the emitted yaml's inline comments are the option reference.

## Commands

```sh
cellstudio tool list                              # tools: segmenter, tracker
cellstudio run <tool> list                        # algorithms + installed versions
cellstudio run <tool> init --algo <name> [-o f]   # emit config yaml
cellstudio run <tool> exec <config.yaml> [--gpu]  # run (CPU default)
```

`cellstudio run <tool> ...` == `uv run --project clis/<tool> <tool> ...` (each tool has its own venv; never use system python or pip). `--help` works after any subcommand. If `cellstudio` is not installed: `uv tool install clis/cellstudio` (add `--editable` when developing the umbrella itself); it locates the repo from the install source, or from `CELLSTUDIO_CLIS_DIR` if the repo moved.

## IO contract

- segmenter input `io.input.image`: tif/ome-tiff, nd2, czi, lif, dv, png/jpg, ome-zarr, npy, a directory, or a glob pattern like /data/*.ims (quote yaml values starting with *). Multiple files = batch mode: each file is segmented independently and written as `<input-stem>_<masks-name>` (or use `{stem}`/`{dir}` placeholders in `io.output.masks`, e.g. "{dir}/{stem}_masks.tif"). Non-tiff formats are read via bioio as TCZYX then squeezed; the `[io]` log line prints the model-input axes and shape with axis indices (e.g. `ZYX (5, 1024, 1024) (Z=axis 0, ...)` — use these for cellpose channel_axis/z_axis), channel names, and pixel sizes; `io.input.scene`/`io.input.channel` (index or name) apply per file. Output masks: label tiff, 0 = background.
- tracker input `io.input.labels`: TYX or TZYX label stack; a directory or glob here is stacked on axis 0 (frames), e.g. the per-frame masks a segmenter batch produced. Outputs: `tracks.csv` (frame, label, track_id, tree_id; centroid mode adds centroid-* columns), laptrack also `splits.csv`/`merges.csv` (parent_track_id, child_track_id), and `tracked_labels.tif` where pixel value = track_id + 1.
- Relative paths in a config resolve against the cwd of `exec`, not the yaml's location.

## Pitfalls

| Symptom                                                                 | Fix                                                                                                                                                                                                                                                                       |
| ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| cellpose finds 0 objects                                                | auto-diameter fails on clean/synthetic images: set `options.eval.diameter`                                                                                                                                                                                                |
| first exec very slow                                                    | pretrained weights download once (cellpose 1.15 GB) into the user cache                                                                                                                                                                                                   |
| `--gpu` on a no-NVIDIA machine                                          | fine: warns and runs on CPU (laptrack/ultrack are CPU-only always)                                                                                                                                                                                                        |
| stardist on a timelapse                                                 | set `options.per_frame: true` (2D model per frame)                                                                                                                                                                                                                        |
| laptrack overlap mode links everything                                  | default `cutoff: 225` accepts any overlapping pair; overlap distances are ~[0,1], use e.g. 0.9                                                                                                                                                                            |
| laptrack centroid cutoff confusion                                      | default metric is sqeuclidean, so `cutoff` is squared px distance (225 = 15 px max jump)                                                                                                                                                                                  |
| ultrack rerun errors / leftovers                                        | it keeps a database in `options.data.working_dir` (default `.ultrack/` in cwd); `overwrite: all` is the default                                                                                                                                                           |
| wrong torch build for the GPU ("does not include kernels for this GPU") | set top-level `cuda: cu126\|cu130` in the config yaml (cu126 covers V100-H100, default; cu130 = Blackwell). Resolution: `CELLSTUDIO_TORCH` env > yaml `cuda:` > `.torch-gpu` marker > NVIDIA detection; standalone `uv run` users pass `--no-group cpu --group gpu-cuXXX` |
| ims/vsi/oir/oib inputs                                                  | `cellstudio run` auto-enables the segmenter's `bioformats` group when the exec config mentions one (persisted in a `.bioformats` marker); standalone `uv run` users pass `--group bioformats` themselves                                                                  |
| segmenting the wrong channel of a multi-channel file                    | set `io.input.channel` (name or index); the `[io]` log line lists the file's channel names                                                                                                                                                                                |

## Extending

New algorithm: `clis/<tool>/src/<tool>/tools/<name>/{config,io,runner}.py` + entry in `src/<tool>/registry.py`: config.py is pydantic models mapping the full API (lightweight imports only), runner.py does heavy imports inside `run(cfg, ctx)`. New tool: copy the `segmenter/` project layout; a console script named after the directory makes `cellstudio tool list` discover it. Details: `clis/README.md`.
