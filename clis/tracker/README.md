# tracker

Cell tracking CLI. Algorithms: laptrack (LAP linking, overlap or centroid), ultrack (ILP over segmentation hypotheses).

```sh
uv run tracker list
uv run tracker init --algo laptrack
uv run tracker exec laptrack.yaml   # TYX/TZYX label stack in -> tracks.csv + relabeled masks out
```

Layout: `src/tracker/tools/<algo>/{config,io,runner}.py`, registered in `src/tracker/registry.py`. See `clis/README.md`.
