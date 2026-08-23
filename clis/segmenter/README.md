# segmenter

Automatic cell segmentation CLI. Algorithms: cellpose (Cellpose-SAM), stardist, micro-sam.

```sh
uv run segmenter list                  # algorithms + versions
uv run segmenter init --algo cellpose  # emit cellpose.yaml
uv run segmenter exec cellpose.yaml    # image in -> label masks out
```

Layout: `src/segmenter/tools/<algo>/{config,io,runner}.py`, registered in `src/segmenter/registry.py`. See `clis/README.md`.
