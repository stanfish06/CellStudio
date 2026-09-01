#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "numcodecs",
#     "numpy",
#     "tifffile",
#     "zarr>=3,<4",
# ]
# ///
"""Convert ultrack output into CellStudio project artifacts.

Reads ``tracks.csv`` and ``tracked_labels.tif`` from an ultrack output dir,
assigns fresh sequential u32 cell ids (sorted by ``(t, track_id)``), rewrites
the tif voxels (voxel value == ``track_id`` -> node id) into a ``labels.zarr``
pyramid mirroring the image OME-Zarr's levels, and writes ``tracking.json.gz``
(cellstudio-tracking v1) beside it. Links derive from ``parent_id``;
``parent_track_id`` is only cross-checked. The ultrack node id is kept as
``features.ultrack_id``.

Example:
    uv run scripts/ultrack_to_cellstudio.py --ultrack-dir .data/F00 \\
        --image .data/260817_EXP63_live_bse_fa100_F00.zarr \\
        --project .data/260817_EXP63_live_bse_fa100_F00.cellstudio
"""

from __future__ import annotations

import argparse
import csv
import gzip
import json
import shutil
import sqlite3
import sys
from collections.abc import Callable
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import numpy as np
import tifffile
import zarr
from numcodecs import Blosc
from zarr.codecs import ZstdCodec

AXES = ("t", "c", "z", "y", "x")
LABEL_CHUNK_Z = 4
LABEL_CHUNK_XY = 128
MAX_LABEL_ID = (1 << 24) - 1
NULL = "-1"
CSV_COLUMNS = ("track_id", "t", "z", "y", "x", "id", "parent_track_id", "parent_id")
MISMATCH_LIST_CAP = 20


@dataclass(frozen=True)
class Node:
    """One cell with its fresh sequential id; ``t`` is the 0-based csv frame."""

    id: int
    ultrack_id: int
    track_id: int
    t: int
    centroid: tuple[float, float, float]  # z, y, x
    parent: int | None  # parent node id


@dataclass(frozen=True)
class ImageMeta:
    root: Path
    zarr_format: int
    level_dims: tuple[tuple[int, int, int, int, int], ...]  # TCZYX per level
    level_scales: tuple[tuple[float, float, float, float, float], ...]


def parse_tracks(csv_path: Path) -> tuple[list[Node], list[str]]:
    """Nodes with fresh ids plus the parent_track_id cross-check failures."""
    with csv_path.open(newline="") as handle:
        reader = csv.DictReader(handle)
        if reader.fieldnames is None or tuple(reader.fieldnames) != CSV_COLUMNS:
            raise SystemExit(
                f"{csv_path}: header {reader.fieldnames} != {list(CSV_COLUMNS)}"
            )
        rows = list(reader)
    rows.sort(key=lambda r: (int(r["t"]), int(r["track_id"])))

    node_of_ultrack: dict[int, int] = {}
    for index, row in enumerate(rows):
        ultrack_id = int(row["id"])
        if ultrack_id in node_of_ultrack:
            raise SystemExit(f"{csv_path}: duplicate id {ultrack_id}")
        node_of_ultrack[ultrack_id] = index + 1

    nodes: list[Node] = []
    for index, row in enumerate(rows):
        parent: int | None = None
        if row["parent_id"] != NULL:
            parent = node_of_ultrack.get(int(row["parent_id"]))
            if parent is None:
                raise SystemExit(
                    f"{csv_path}: row id {row['id']} names parent_id "
                    f"{row['parent_id']} which has no row"
                )
        nodes.append(
            Node(
                id=index + 1,
                ultrack_id=int(row["id"]),
                track_id=int(row["track_id"]),
                t=int(row["t"]),
                centroid=(float(row["z"]), float(row["y"]), float(row["x"])),
                parent=parent,
            )
        )

    # Cross-check: parent_track_id must be constant per track and equal the
    # track of the head row's parent (-1 for roots). Links never derive from it.
    ptids: dict[int, set[int]] = {}
    heads: dict[int, Node] = {}
    by_id = {n.id: n for n in nodes}
    for node, row in zip(nodes, rows, strict=True):
        ptids.setdefault(node.track_id, set()).add(int(row["parent_track_id"]))
        head = heads.get(node.track_id)
        if head is None or node.t < head.t:
            heads[node.track_id] = node
    failures: list[str] = []
    for track_id in sorted(ptids):
        declared = ptids[track_id]
        if len(declared) > 1:
            failures.append(
                f"track {track_id}: parent_track_id varies across rows: "
                f"{sorted(declared)}"
            )
            continue
        head = heads[track_id]
        expected = by_id[head.parent].track_id if head.parent is not None else -1
        if declared != {expected}:
            failures.append(
                f"track {track_id}: parent_track_id {declared.pop()} but head "
                f"parent is in track {expected}"
            )
    return nodes, failures


def read_image_meta(path: Path) -> ImageMeta:
    group = zarr.open_group(path, mode="r")
    attrs = dict(group.attrs)
    ome = attrs.get("ome", attrs)
    entry = ome["multiscales"][0]
    axes = tuple(a["name"] for a in entry["axes"])
    if axes != AXES:
        raise SystemExit(f"{path}: image axes {axes} != {AXES}")
    dims: list[tuple[int, int, int, int, int]] = []
    scales: list[tuple[float, float, float, float, float]] = []
    for level, dataset in enumerate(entry["datasets"]):
        array = group[dataset["path"]]
        t, c, z, y, x = (int(n) for n in array.shape)
        dims.append((t, c, z, y, x))
        scale = next(
            (
                tr["scale"]
                for tr in dataset.get("coordinateTransformations", [])
                if tr["type"] == "scale"
            ),
            None,
        )
        if scale is None:
            z0, y0, x0 = dims[0][2], dims[0][3], dims[0][4]
            scale = [1.0, 1.0, z0 / z, y0 / y, x0 / x]
        scales.append((1.0, 1.0, float(scale[2]), float(scale[3]), float(scale[4])))
    return ImageMeta(
        root=path,
        zarr_format=int(group.metadata.zarr_format),
        level_dims=tuple(dims),
        level_scales=tuple(scales),
    )


def probe_project_lock(project: Path) -> None:
    """The app holds a zero-timeout exclusive lock on tracks.sqlite while open."""
    db = project / "tracks.sqlite"
    if not db.exists():
        return
    connection = sqlite3.connect(db, timeout=0)
    try:
        connection.execute("PRAGMA busy_timeout = 0")
        connection.execute("BEGIN IMMEDIATE")
        connection.execute("ROLLBACK")
    except sqlite3.OperationalError:
        raise SystemExit(f"{db} is locked; close the app first") from None
    finally:
        connection.close()


def label_attributes(meta: ImageMeta) -> dict[str, Any]:
    entry: dict[str, Any] = {
        "name": "cellstudio-labels",
        "axes": [
            {"name": "t", "type": "time", "unit": "second"},
            {"name": "c", "type": "channel"},
            {"name": "z", "type": "space", "unit": "micrometer"},
            {"name": "y", "type": "space", "unit": "micrometer"},
            {"name": "x", "type": "space", "unit": "micrometer"},
        ],
        "datasets": [
            {
                "path": str(level),
                "coordinateTransformations": [{"type": "scale", "scale": list(scale)}],
            }
            for level, scale in enumerate(meta.level_scales)
        ],
    }
    if meta.zarr_format == 2:
        entry["version"] = "0.4"
        return {"multiscales": [entry], "cellstudio_labels": str(meta.root)}
    return {
        "ome": {"version": "0.5", "multiscales": [entry]},
        "cellstudio_labels": str(meta.root),
    }


def create_label_arrays(tmp: Path, meta: ImageMeta) -> list[zarr.Array]:
    if meta.zarr_format == 2:
        compressors: Any = Blosc(cname="zstd", clevel=5, shuffle=Blosc.SHUFFLE)
    else:
        compressors = ZstdCodec(level=3)
    root = zarr.create_group(tmp, overwrite=True, zarr_format=meta.zarr_format)
    root.attrs.update(label_attributes(meta))
    arrays: list[zarr.Array] = []
    for level, (t, _c, z, y, x) in enumerate(meta.level_dims):
        shape = (t, 1, z, y, x)
        chunks = (
            1,
            1,
            min(LABEL_CHUNK_Z, z),
            min(LABEL_CHUNK_XY, y),
            min(LABEL_CHUNK_XY, x),
        )
        array = root.create_array(
            str(level),
            shape=shape,
            chunks=chunks,
            dtype=np.uint32,
            compressors=compressors,
            fill_value=0,
            dimension_names=list(AXES) if meta.zarr_format == 3 else None,
        )
        if meta.zarr_format == 2:
            array.attrs["_ARRAY_DIMENSIONS"] = list(AXES)
        arrays.append(array)
    return arrays


def rewrite_frames(
    tif_path: Path,
    nodes: list[Node],
    *,
    t_offset: int,
    on_frame: Callable[[int, np.ndarray], None] | None,
) -> None:
    """Remap each tif frame's voxel value (== track_id) to the node id there.

    ``on_frame(dataset_t, u32 volume)`` receives every remapped frame; None
    validates only (dry run). A tif value with no csv row at that frame, or
    vice versa, aborts listing the offending pairs.
    """
    node_at: dict[int, dict[int, int]] = {}
    for node in nodes:
        node_at.setdefault(node.t, {})[node.track_id] = node.id

    mismatches: list[tuple[int, int, str]] = []  # (csv frame, tif value, side)
    with tifffile.TiffFile(tif_path) as tif:
        series = tif.series[0]
        n_frames = int(series.shape[0])
        if nodes and nodes[-1].t >= n_frames:
            raise SystemExit(
                f"{tif_path}: {n_frames} frame(s) but tracks.csv reaches "
                f"t={nodes[-1].t}"
            )
        for t in range(n_frames):
            page = series.asarray(key=t)
            values = node_at.get(t, {})
            present = {int(v) for v in np.unique(page)} - {0}
            for value in sorted(present - values.keys()):
                mismatches.append((t, value, "tif value has no csv row"))
            for value in sorted(values.keys() - present):
                mismatches.append((t, value, "csv row has no tif voxels"))
            if mismatches or on_frame is None:
                continue
            lut = np.zeros(int(page.max()) + 1, dtype=np.uint32)
            for value, node_id in values.items():
                lut[value] = node_id
            on_frame(t + t_offset, lut[page])
    if mismatches:
        listed = "\n".join(
            f"  t={t} value=track_id={value}: {side}"
            for t, value, side in mismatches[:MISMATCH_LIST_CAP]
        )
        more = len(mismatches) - MISMATCH_LIST_CAP
        raise SystemExit(
            f"{tif_path} and tracks.csv disagree on {len(mismatches)} "
            f"(frame, value) pair(s):\n{listed}"
            + (f"\n  ... and {more} more" if more > 0 else "")
        )


def write_label_pyramid(
    tmp: Path, meta: ImageMeta, tif_path: Path, nodes: list[Node], t_offset: int
) -> None:
    arrays = create_label_arrays(tmp, meta)
    _t0, _c0, z0, y0, x0 = meta.level_dims[0]
    # nearest-neighbour stride indices from level 0 to each level's exact dims
    strides = [
        tuple(
            (np.arange(n) * (full / n)).astype(np.int64)
            for full, n in ((z0, z), (y0, y), (x0, x))
        )
        for _t, _c, z, y, x in meta.level_dims[1:]
    ]
    frames_done = 0

    def on_frame(t: int, volume: np.ndarray) -> None:
        nonlocal frames_done
        arrays[0][t, 0] = volume
        for array, (iz, iy, ix) in zip(arrays[1:], strides, strict=True):
            array[t, 0] = volume[np.ix_(iz, iy, ix)]
        frames_done += 1
        if frames_done == 1 or frames_done % 25 == 0:
            print(f"  frame {frames_done}  t={t}", flush=True)

    rewrite_frames(tif_path, nodes, t_offset=t_offset, on_frame=on_frame)
    print(f"  {frames_done} frame(s) written")


def verify_store(tmp: Path, meta: ImageMeta, max_id: int) -> None:
    """The contract rules the app enforces, checked before the rename."""
    if max_id > MAX_LABEL_ID:
        raise SystemExit(f"max assigned id {max_id} exceeds {MAX_LABEL_ID}")
    group = zarr.open_group(tmp, mode="r")
    if int(group.metadata.zarr_format) != meta.zarr_format:
        raise SystemExit(
            f"store is zarr v{group.metadata.zarr_format}, image is v{meta.zarr_format}"
        )
    names = sorted(group.array_keys(), key=int)
    if names != [str(i) for i in range(len(meta.level_dims))]:
        raise SystemExit(
            f"store levels {names} != image's {len(meta.level_dims)} level(s)"
        )
    for level, (t, _c, z, y, x) in enumerate(meta.level_dims):
        array = group[str(level)]
        if array.dtype != np.uint32:
            raise SystemExit(f"level {level} dtype {array.dtype} != uint32")
        if tuple(array.shape) != (t, 1, z, y, x):
            raise SystemExit(
                f"level {level} shape {tuple(array.shape)} != {(t, 1, z, y, x)}"
            )
        dim_names = (
            array.metadata.dimension_names
            if meta.zarr_format == 3
            else array.attrs.get("_ARRAY_DIMENSIONS")
        )
        if tuple(dim_names or ()) != AXES:
            raise SystemExit(f"level {level} axes {dim_names} != {list(AXES)}")


def tracking_document(
    nodes: list[Node], meta: ImageMeta, ultrack_dir: Path, t_offset: int
) -> dict[str, Any]:
    children: dict[int, list[int]] = {}
    for node in nodes:
        if node.parent is not None:
            children.setdefault(node.parent, []).append(node.id)
    t, _c, z, y, x = meta.level_dims[0]
    return {
        "format": "cellstudio-tracking",
        "version": 1,
        "metadata": {
            "created": datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "source": (f"ultrack_to_cellstudio.py: {ultrack_dir}, t_offset={t_offset}"),
            "shape_tczyx": [t, 1, z, y, x],
        },
        "cells": [
            {
                "id": node.id,
                "t": node.t + t_offset,
                "track_id": node.track_id,
                "centroid": list(node.centroid),
                "children": [{"id": child} for child in children.get(node.id, [])],
                "features": {"ultrack_id": node.ultrack_id},
            }
            for node in nodes
        ],
    }


def convert(
    ultrack_dir: Path,
    image: Path,
    project: Path,
    *,
    t_offset: int,
    replace: bool,
    dry_run: bool,
) -> None:
    csv_path = ultrack_dir / "tracks.csv"
    tif_path = ultrack_dir / "tracked_labels.tif"
    for path in (csv_path, tif_path):
        if not path.is_file():
            raise SystemExit(f"not a file: {path}")

    meta = read_image_meta(image)
    nodes, failures = parse_tracks(csv_path)
    if not nodes:
        raise SystemExit(f"{csv_path}: no rows")
    tracks = {n.track_id for n in nodes}
    roots = sum(1 for n in nodes if n.parent is None)
    t_min, t_max = nodes[0].t, nodes[-1].t
    image_t = meta.level_dims[0][0]
    if t_offset < 0:
        raise SystemExit(f"--t-offset {t_offset} is negative")
    if t_offset + t_max + 1 > image_t:
        raise SystemExit(
            f"--t-offset {t_offset} + {t_max + 1} csv frame(s) exceeds image T="
            f"{image_t}"
        )

    if dry_run:
        rewrite_frames(tif_path, nodes, t_offset=t_offset, on_frame=None)
        print(
            f"dry run (no writes)\n"
            f"  cells: {len(nodes)}\n"
            f"  tracks: {len(tracks)}\n"
            f"  roots: {roots}\n"
            f"  cross-check failures: {len(failures)}\n"
            f"  frame range: csv {t_min}..{t_max} -> dataset "
            f"{t_min + t_offset}..{t_max + t_offset} (image T={image_t})\n"
            f"  max assigned id: {nodes[-1].id}"
        )
        for failure in failures[:MISMATCH_LIST_CAP]:
            print(f"  cross-check: {failure}")
        return

    if failures:
        listed = "\n".join(f"  {f}" for f in failures[:MISMATCH_LIST_CAP])
        raise SystemExit(
            f"{csv_path}: {len(failures)} parent_track_id mismatch(es):\n{listed}"
        )

    store_path = project / "labels.zarr"
    json_path = project / "tracking.json.gz"
    if store_path.exists() and not replace:
        raise SystemExit(f"{store_path} exists (pass --replace)")
    project.mkdir(parents=True, exist_ok=True)
    probe_project_lock(project)

    tmp_store = project / ".labels.zarr.converting"
    tmp_json = project / ".tracking.json.gz.converting"
    for tmp in (tmp_store, tmp_json):
        if tmp.exists():
            shutil.rmtree(tmp) if tmp.is_dir() else tmp.unlink()

    print(
        f"writing {store_path}\n"
        f"  cells={len(nodes)} tracks={len(tracks)} roots={roots} "
        f"levels={len(meta.level_dims)} zarr_format={meta.zarr_format}"
    )
    try:
        write_label_pyramid(tmp_store, meta, tif_path, nodes, t_offset)
        verify_store(tmp_store, meta, nodes[-1].id)
        doc = tracking_document(nodes, meta, ultrack_dir, t_offset)
        tmp_json.write_bytes(gzip.compress(json.dumps(doc).encode(), mtime=0))
    except BaseException:
        shutil.rmtree(tmp_store, ignore_errors=True)
        tmp_json.unlink(missing_ok=True)
        raise
    if store_path.exists():
        shutil.rmtree(store_path)
    tmp_store.rename(store_path)
    tmp_json.replace(json_path)
    print(f"done  {store_path}\n      {json_path}")


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--ultrack-dir",
        type=Path,
        required=True,
        help="Dir with tracks.csv and tracked_labels.tif",
    )
    parser.add_argument(
        "--image",
        type=Path,
        default=None,
        help="Image OME-Zarr (default: the project's project.json source)",
    )
    parser.add_argument(
        "--project",
        type=Path,
        required=True,
        help="<dataset>.cellstudio project dir (created if absent)",
    )
    parser.add_argument(
        "--t-offset",
        type=int,
        default=0,
        help="Dataset frame that csv/tif frame 0 maps to (default 0)",
    )
    parser.add_argument(
        "--replace", action="store_true", help="Replace an existing labels.zarr"
    )
    parser.add_argument(
        "--dry-run", action="store_true", help="Validate and report, write nothing"
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> None:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    project = args.project.expanduser().resolve()
    image = args.image
    if image is None:
        project_json = project / "project.json"
        if not project_json.is_file():
            raise SystemExit(f"no --image and no {project_json}")
        image = Path(json.loads(project_json.read_text())["source"])
    image = image.expanduser().resolve()
    if not image.is_dir():
        raise SystemExit(f"not a directory: {image}")
    convert(
        args.ultrack_dir.expanduser().resolve(),
        image,
        project,
        t_offset=args.t_offset,
        replace=args.replace,
        dry_run=args.dry_run,
    )


if __name__ == "__main__":
    main()
