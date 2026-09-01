Helper scripts for data conversion/generation.

- `ultrack_to_cellstudio.py` — convert ultrack output (`tracks.csv` +
  `tracked_labels.tif`, voxel value = `track_id`) into `<project>/labels.zarr`
  (u32 TCZYX pyramid mirroring the image levels, chunks 1x1x4x128x128, zstd)
  and `<project>/tracking.json.gz` (cellstudio-tracking v1). Assigns fresh
  sequential ids sorted by `(t, track_id)`, keeps the ultrack id as
  `features.ultrack_id`, links from `parent_id` only. Writes to temp siblings,
  self-verifies the store contract, then renames; refuses an existing
  `labels.zarr` without `--replace` and refuses while the app holds the
  project's `tracks.sqlite` lock. `--dry-run` reports counts without writing.

  ```
  uv run scripts/ultrack_to_cellstudio.py --ultrack-dir .data/F00 \
      --image .data/260817_EXP63_live_bse_fa100_F00.zarr \
      --project .data/260817_EXP63_live_bse_fa100_F00.cellstudio
  ```
