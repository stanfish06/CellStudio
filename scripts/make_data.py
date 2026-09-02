#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "numcodecs",
#     "numpy",
#     "zarr>=3,<4",
# ]
# ///
"""Generate the tiny correctness data for CellStudio's dataset-io tests.

Each store lands in its own subdirectory of the output dir together with a
one-line ``MANIFEST``; ``manifest.json`` at the top level indexes them all.
Everything is KB-to-MB and regenerated on demand; the output dir is
gitignored, never committed.

The image, label, and tracking stores describe the *same* 4-frame, 6-nucleus
scene from one fixed seed: masks align with ``tiny_v2``/``tiny_v3``
voxel-for-voxel, and the tracking JSON's ``seg_id``s resolve in
``labels_background0`` while its centroids and areas equal the ones a mask
importer computes. That makes cross-store integration tests meaningful.

Regeneration replaces store directories in place, so the command is safe to
re-run; a directory without a ``MANIFEST`` is never deleted.

Example:
    uv run scripts/make_data.py --out .data
    uv run scripts/make_data.py --out .data --only tiny_v3
    uv run scripts/make_data.py --out .data --verify-only
"""

from __future__ import annotations

import argparse
import gzip
import json
import math
import shutil
import sys
from collections.abc import Callable, Iterator, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import numpy as np
import zarr
from numcodecs import Blosc
from zarr.codecs import BloscCodec

AXES_TCZYX = ("t", "c", "z", "y", "x")
AXES_TZYX = ("t", "z", "y", "x")

SCENE_SEED = 20260821
NOISE_SEED = 7
TINY_T, TINY_C, TINY_Z, TINY_Y, TINY_X = 4, 2, 4, 32, 32
TINY_BLOBS = 6
HOSTILE_T, HOSTILE_Z, HOSTILE_Y, HOSTILE_X = 2, 64, 64, 64
UINT32_MAX = 2**32 - 1

TINY_VOXEL_ZYX = (2.0, 0.5, 0.5)
TINY_DT_S = 60.0


@dataclass(frozen=True)
class Blob:
    """One nucleus-like ellipsoid in voxel coordinates."""

    z: float
    y: float
    x: float
    sigma_z: float
    sigma_yx: float
    amp: float


def scene(
    n_frames: int, n_blobs: int, shape_zyx: tuple[int, int, int]
) -> list[list[Blob]]:
    """Nuclei drifting on a random walk: one blob list per frame, fixed seed."""
    z, y, x = shape_zyx
    rng = np.random.default_rng(SCENE_SEED)
    margin = 3.0
    current = [
        Blob(
            z=float(rng.uniform(margin * 0.25, z - margin * 0.25)),
            y=float(rng.uniform(margin, y - margin)),
            x=float(rng.uniform(margin, x - margin)),
            sigma_z=0.9,
            sigma_yx=1.8 + 0.4 * i / max(1, n_blobs - 1),
            amp=0.55 + 0.45 * (i % 3) / 2.0,
        )
        for i in range(n_blobs)
    ]
    frames = [current]
    for _ in range(n_frames - 1):
        current = [
            Blob(
                z=float(np.clip(b.z + rng.normal(0.0, 0.2), 0.5, z - 1.5)),
                y=float(np.clip(b.y + rng.normal(0.0, 1.1), margin, y - margin)),
                x=float(np.clip(b.x + rng.normal(0.0, 1.1), margin, x - margin)),
                sigma_z=b.sigma_z,
                sigma_yx=b.sigma_yx,
                amp=b.amp,
            )
            for b in current
        ]
        frames.append(current)
    return frames


def blob_field(
    blobs: Sequence[Blob], shape_zyx: tuple[int, int, int], sigma_gain: float = 1.0
) -> np.ndarray:
    """Sum of anisotropic 3D gaussians, peak-normalized to ~1."""
    z, y, x = shape_zyx
    gz = np.arange(z, dtype=np.float32)[:, None, None]
    gy = np.arange(y, dtype=np.float32)[None, :, None]
    gx = np.arange(x, dtype=np.float32)[None, None, :]
    field = np.zeros(shape_zyx, dtype=np.float32)
    for b in blobs:
        sz = b.sigma_z * sigma_gain
        sxy = b.sigma_yx * sigma_gain
        field += b.amp * np.exp(
            -(
                ((gz - b.z) ** 2) / (2.0 * sz * sz)
                + ((gy - b.y) ** 2) / (2.0 * sxy * sxy)
                + ((gx - b.x) ** 2) / (2.0 * sxy * sxy)
            )
        )
    return field


def to_uint16(
    field: np.ndarray, rng: np.random.Generator, *, offset: int, signal: int
) -> np.ndarray:
    """Blob field + illumination gradient + shot noise, clipped to uint16."""
    _, y, x = field.shape
    ramp = (
        np.linspace(0.0, 1.0, y, dtype=np.float32)[None, :, None]
        * np.linspace(0.0, 1.0, x, dtype=np.float32)[None, None, :]
    )
    counts = offset + 0.12 * signal * ramp + signal * field
    noisy = rng.poisson(np.clip(counts, 0.0, None)).astype(np.float32)
    return np.clip(noisy, 0, 65535).astype(np.uint16)


def label_volume(
    blobs: Sequence[Blob], shape_zyx: tuple[int, int, int], ids: Sequence[int]
) -> np.ndarray:
    """Ellipsoid cores painted with `ids`; 0 is background. Lower index wins ties."""
    out = np.zeros(shape_zyx, dtype=np.uint32)
    z, y, x = shape_zyx
    gz = np.arange(z, dtype=np.float32)[:, None, None]
    gy = np.arange(y, dtype=np.float32)[None, :, None]
    gx = np.arange(x, dtype=np.float32)[None, None, :]
    for label, b in zip(reversed(ids), reversed(blobs), strict=True):
        inside = (
            ((gz - b.z) / (1.4 * b.sigma_z)) ** 2
            + ((gy - b.y) / (1.5 * b.sigma_yx)) ** 2
            + ((gx - b.x) / (1.5 * b.sigma_yx)) ** 2
        ) <= 1.0
        out[inside] = label
    return out


def region_stats(labels: np.ndarray, label: int) -> tuple[list[float], int]:
    """Centroid [z,y,x] and voxel count for one label value."""
    zz, yy, xx = np.nonzero(labels == label)
    if zz.size == 0:
        raise ValueError(f"label {label} absent from volume")
    centroid = [round(float(a.mean()), 4) for a in (zz, yy, xx)]
    return centroid, int(zz.size)


def downsample_yx(volume: np.ndarray) -> np.ndarray:
    """Mean-pool ZYX by 2 in Y and X, matching scripts/ims_to_zarr.py."""
    z, y, x = volume.shape
    y2, x2 = y // 2, x // 2
    if y2 == 0 or x2 == 0:
        raise ValueError(f"cannot downsample shape {volume.shape}")
    pooled = volume[:, : y2 * 2, : x2 * 2].reshape(z, y2, 2, x2, 2).mean(axis=(2, 4))
    return np.rint(pooled).astype(volume.dtype, copy=False)


def multiscale_datasets(
    n_levels: int,
    *,
    axes: Sequence[str],
    dt_s: float,
    voxel_zyx: tuple[float, float, float],
    with_transforms: bool = True,
) -> list[dict[str, Any]]:
    dz, dy, dx = voxel_zyx
    datasets: list[dict[str, Any]] = []
    for level in range(n_levels):
        entry: dict[str, Any] = {"path": str(level)}
        if with_transforms:
            factor = 2**level
            scale = {"t": dt_s, "c": 1.0, "z": dz, "y": dy * factor, "x": dx * factor}
            entry["coordinateTransformations"] = [
                {"type": "scale", "scale": [scale[a] for a in axes]}
            ]
        datasets.append(entry)
    return datasets


def axis_entries(axes: Sequence[str]) -> list[dict[str, str]]:
    spec = {
        "t": {"name": "t", "type": "time", "unit": "second"},
        "c": {"name": "c", "type": "channel"},
        "z": {"name": "z", "type": "space", "unit": "micrometer"},
        "y": {"name": "y", "type": "space", "unit": "micrometer"},
        "x": {"name": "x", "type": "space", "unit": "micrometer"},
    }
    return [spec[a] for a in axes]


def omero_block(
    name: str, channels: Sequence[tuple[str, str, tuple[int, int]]], default_z: int
) -> dict[str, Any]:
    return {
        "id": 1,
        "name": name,
        "version": "0.4",
        "channels": [
            {
                "active": True,
                "coefficient": 1.0,
                "color": color,
                "family": "linear",
                "inverted": False,
                "label": label,
                "window": {
                    "min": 0.0,
                    "max": 65535.0,
                    "start": float(start),
                    "end": float(end),
                },
            }
            for label, color, (start, end) in channels
        ],
        "rdefs": {"defaultT": 0, "defaultZ": default_z, "model": "color"},
    }


def chunk_shape(
    shape: tuple[int, ...], axes: Sequence[str], chunk_z: int, tile: int
) -> tuple[int, ...]:
    limits = {"t": 1, "c": 1, "z": chunk_z, "y": tile, "x": tile}
    return tuple(min(limits[a], n) for a, n in zip(axes, shape, strict=True))


def read_amplification(
    shape_zyx: tuple[int, int, int], chunk_zyx: tuple[int, int, int]
) -> dict[str, float]:
    """Uncompressed bytes a whole-plane read touches / bytes of the plane."""
    z, y, x = shape_zyx
    cz, cy, cx = chunk_zyx

    def span(n: int, c: int) -> int:
        return math.ceil(n / c) * c

    return {
        "xy": cz * span(y, cy) * span(x, cx) / (y * x),
        "xz": span(z, cz) * cy * span(x, cx) / (z * x),
        "yz": span(z, cz) * span(y, cy) * cx / (z * y),
    }


def write_store(
    path: Path,
    *,
    zarr_format: int,
    axes: Sequence[str],
    levels: Sequence[np.ndarray],
    chunk_z: int,
    tile: int,
    attrs: dict[str, Any],
) -> None:
    """Write one multiscale OME-Zarr group, v2 or v3, blosc-zstd clevel 5."""
    if zarr_format == 2:
        compressors: Any = Blosc(cname="zstd", clevel=5, shuffle=Blosc.SHUFFLE)
    else:
        compressors = BloscCodec(cname="zstd", clevel=5, shuffle="shuffle", typesize=2)
    root = zarr.create_group(path, overwrite=True, zarr_format=zarr_format)
    for index, data in enumerate(levels):
        array = root.create_array(
            str(index),
            shape=data.shape,
            chunks=chunk_shape(data.shape, axes, chunk_z, tile),
            dtype=data.dtype,
            compressors=compressors,
            fill_value=0,
            dimension_names=list(axes) if zarr_format == 3 else None,
        )
        array[...] = data
        if zarr_format == 2:
            array.attrs["_ARRAY_DIMENSIONS"] = list(axes)
    root.attrs.update(attrs)


def image_levels(
    frames: Sequence[Sequence[Blob]],
    *,
    n_levels: int,
    shape_zyx: tuple[int, int, int],
    channels: int,
) -> list[np.ndarray]:
    """Level-0 TCZYX plus an XY-only pyramid; channel 1 is a wider, dimmer marker."""
    rng = np.random.default_rng(NOISE_SEED)
    z, y, x = shape_zyx
    level0 = np.zeros((len(frames), channels, z, y, x), dtype=np.uint16)
    for t, blobs in enumerate(frames):
        for c in range(channels):
            field = blob_field(blobs, shape_zyx, sigma_gain=1.0 + 0.6 * c)
            level0[t, c] = to_uint16(
                field, rng, offset=110, signal=6000 if c == 0 else 2400
            )
    out = [level0]
    for _ in range(n_levels - 1):
        prev = out[-1]
        nxt = np.stack(
            [
                np.stack([downsample_yx(prev[t, c]) for c in range(channels)])
                for t in range(prev.shape[0])
            ]
        )
        out.append(nxt)
    return out


TINY_CHANNELS = (
    ("mNeonGreen-H2B", "37FF00", (120, 5200)),
    ("mScarlet-membrane", "FF0000", (120, 2100)),
)


def tiny_attrs(zarr_format: int, n_levels: int) -> dict[str, Any]:
    axes = AXES_TCZYX
    entry: dict[str, Any] = {
        "name": "tiny",
        "axes": axis_entries(axes),
        "datasets": multiscale_datasets(
            n_levels, axes=axes, dt_s=TINY_DT_S, voxel_zyx=TINY_VOXEL_ZYX
        ),
        "type": "local_mean",
    }
    omero = omero_block("tiny", TINY_CHANNELS, default_z=TINY_Z // 2)
    if zarr_format == 2:
        entry["version"] = "0.4"
        entry["metadata"] = {"method": "local_mean", "version": "0.4"}
        return {"multiscales": [entry], "omero": omero}
    omero = {**omero, "version": "0.5"}
    return {"ome": {"version": "0.5", "multiscales": [entry], "omero": omero}}


def build_tiny(out: Path, zarr_format: int) -> str:
    frames = scene(TINY_T, TINY_BLOBS, (TINY_Z, TINY_Y, TINY_X))
    levels = image_levels(
        frames, n_levels=3, shape_zyx=(TINY_Z, TINY_Y, TINY_X), channels=TINY_C
    )
    write_store(
        out / "image.zarr",
        zarr_format=zarr_format,
        axes=AXES_TCZYX,
        levels=levels,
        chunk_z=TINY_Z,
        tile=16,
        attrs=tiny_attrs(zarr_format, len(levels)),
    )
    ngff = "NGFF 0.4 attrs at the group root" if zarr_format == 2 else "NGFF 0.5 `ome`"
    return (
        f"Valid tiny multiscale OME-Zarr, zarr v{zarr_format} ({ngff}): TCZYX "
        f"{TINY_T}x{TINY_C}x{TINY_Z}x{TINY_Y}x{TINY_X} uint16, 3 XY-only levels "
        f"(32/16/8), omero 2 channels, voxel 2.0x0.5x0.5 um, same scene as its "
        f"v2/v3 sibling, labels_*, and tracking_valid."
    )


def build_no_scale_metadata(out: Path) -> str:
    frames = scene(TINY_T, TINY_BLOBS, (TINY_Z, TINY_Y, TINY_X))
    levels = image_levels(
        frames, n_levels=2, shape_zyx=(TINY_Z, TINY_Y, TINY_X), channels=TINY_C
    )
    attrs = {
        "multiscales": [
            {
                "version": "0.4",
                "name": "no_scale_metadata",
                "axes": axis_entries(AXES_TCZYX),
                "datasets": multiscale_datasets(
                    len(levels),
                    axes=AXES_TCZYX,
                    dt_s=TINY_DT_S,
                    voxel_zyx=TINY_VOXEL_ZYX,
                    with_transforms=False,
                ),
            }
        ]
    }
    write_store(
        out / "image.zarr",
        zarr_format=2,
        axes=AXES_TCZYX,
        levels=levels,
        chunk_z=TINY_Z,
        tile=16,
        attrs=attrs,
    )
    return (
        "Non-conformant store with no coordinateTransformations and no omero: voxel "
        "size must fall back to isotropic with a warning, level factors must be "
        "inferred from shapes, channels must take default palette colors and "
        "full-range windows."
    )


def hostile_levels() -> list[np.ndarray]:
    frames = scene(HOSTILE_T, 24, (HOSTILE_Z, HOSTILE_Y, HOSTILE_X))
    return image_levels(
        frames, n_levels=1, shape_zyx=(HOSTILE_Z, HOSTILE_Y, HOSTILE_X), channels=1
    )


def build_hostile(out: Path, *, chunk_z: int, tile: int, kind: str) -> str:
    levels = hostile_levels()
    attrs = {
        "multiscales": [
            {
                "version": "0.4",
                "name": kind,
                "axes": axis_entries(AXES_TCZYX),
                "datasets": multiscale_datasets(
                    1, axes=AXES_TCZYX, dt_s=TINY_DT_S, voxel_zyx=(1.0, 0.25, 0.25)
                ),
            }
        ]
    }
    write_store(
        out / "image.zarr",
        zarr_format=2,
        axes=AXES_TCZYX,
        levels=levels,
        chunk_z=chunk_z,
        tile=tile,
        attrs=attrs,
    )
    amp = read_amplification(
        (HOSTILE_Z, HOSTILE_Y, HOSTILE_X), (min(chunk_z, HOSTILE_Z), tile, tile)
    )
    target = "XY" if kind == "hostile_zbrick" else "XZ/YZ"
    return (
        f"Hostile chunk layout ({kind.removeprefix('hostile_')}): ZYX "
        f"{HOSTILE_Z}x{HOSTILE_Y}x{HOSTILE_X} uint16, chunks "
        f"(1,1,{min(chunk_z, HOSTILE_Z)},{tile},{tile}), amplification xy="
        f"{amp['xy']:.0f}x xz={amp['xz']:.0f}x yz={amp['yz']:.0f}x, so the layout "
        f"advisory must fire for {target}."
    )


def label_ids(frame: int, n_blobs: int, *, reuse: bool) -> list[int]:
    return (
        [i + 1 for i in range(n_blobs)]
        if reuse
        else [100 * frame + i + 1 for i in range(n_blobs)]
    )


def build_labels(out: Path, *, reuse: bool) -> str:
    frames = scene(TINY_T, TINY_BLOBS, (TINY_Z, TINY_Y, TINY_X))
    volume = np.zeros((TINY_T, TINY_Z, TINY_Y, TINY_X), dtype=np.uint32)
    for t, blobs in enumerate(frames):
        volume[t] = label_volume(
            blobs, (TINY_Z, TINY_Y, TINY_X), label_ids(t, TINY_BLOBS, reuse=reuse)
        )
    attrs = {
        "multiscales": [
            {
                "version": "0.4",
                "name": "labels",
                "axes": axis_entries(AXES_TZYX),
                "datasets": multiscale_datasets(
                    1, axes=AXES_TZYX, dt_s=TINY_DT_S, voxel_zyx=TINY_VOXEL_ZYX
                ),
            }
        ],
        "image-label": {
            "version": "0.4",
            "source": {"image": "../../tiny_v2/image.zarr"},
            "properties": [
                {"label-value": int(v)}
                for v in sorted(
                    {
                        v
                        for t in range(TINY_T)
                        for v in label_ids(t, TINY_BLOBS, reuse=reuse)
                    }
                )
            ],
        },
    }
    write_store(
        out / "labels.zarr",
        zarr_format=2,
        axes=AXES_TZYX,
        levels=[volume],
        chunk_z=TINY_Z,
        tile=16,
        attrs=attrs,
    )
    unique = int(np.unique(volume).size)
    if reuse:
        return (
            f"Label masks reusing ids 1..{TINY_BLOBS} in every frame (TZYX "
            f"{TINY_T}x{TINY_Z}x{TINY_Y}x{TINY_X} uint32, {unique - 1} distinct "
            f"non-zero values total): import must remap each (frame, label) to a "
            f"distinct global uint32 and record the mapping. Same geometry as "
            f"labels_background0."
        )
    return (
        f"Label masks with globally unique ids and background 0 (TZYX "
        f"{TINY_T}x{TINY_Z}x{TINY_Y}x{TINY_X} uint32, {unique - 1} non-zero values, "
        f"ids 100*t+1..): no cell record may be created for value 0. Same geometry as "
        f"labels_reused_ids."
    )


def tracking_graph() -> list[dict[str, Any]]:
    """D12 records over the tiny scene: a chain per nucleus, one two-child division.

    Blob 0 divides at t=1 into blob 0 and blob 5 of t=2; blob 5 of t=1 dies.
    `seg_id`s are the values in labels_background0, centroids and areas are the
    ones a mask importer computes from it.
    """
    frames = scene(TINY_T, TINY_BLOBS, (TINY_Z, TINY_Y, TINY_X))
    volumes = [
        label_volume(
            blobs, (TINY_Z, TINY_Y, TINY_X), label_ids(t, TINY_BLOBS, reuse=False)
        )
        for t, blobs in enumerate(frames)
    ]

    def cell_id(t: int, j: int) -> int:
        return TINY_BLOBS * t + j + 1

    parents: dict[int, int] = {}
    for t in range(TINY_T - 1):
        for j in range(TINY_BLOBS):
            if t == 1 and j == 5:
                continue
            if t == 1 and j == 0:
                parents[cell_id(2, 0)] = cell_id(1, 0)
                parents[cell_id(2, 5)] = cell_id(1, 0)
                continue
            parents[cell_id(t + 1, j)] = cell_id(t, j)

    children: dict[int, list[int]] = {}
    for child, parent in sorted(parents.items()):
        children.setdefault(parent, []).append(child)

    def confidence(parent: int, child: int) -> float:
        return round(0.80 + 0.0037 * ((parent * 31 + child * 17) % 54), 3)

    track_of: dict[int, int] = {}
    next_track = 1
    for t in range(TINY_T):
        for j in range(TINY_BLOBS):
            cid = cell_id(t, j)
            parent = parents.get(cid)
            if parent is not None and len(children[parent]) == 1:
                track_of[cid] = track_of[parent]
            else:
                track_of[cid] = next_track
                next_track += 1

    records: list[dict[str, Any]] = []
    for t in range(TINY_T):
        for j in range(TINY_BLOBS):
            cid = cell_id(t, j)
            seg = label_ids(t, TINY_BLOBS, reuse=False)[j]
            centroid, area = region_stats(volumes[t], seg)
            kids = children.get(cid, [])
            record: dict[str, Any] = {
                "id": cid,
                "t": t,
                "seg_id": seg,
                "track_id": track_of[cid],
                "centroid": centroid,
                "children": [{"id": k, "confidence": confidence(cid, k)} for k in kids],
                "confidence": round(0.90 + 0.001 * (cid % 90), 3),
                "features": {"area": area, "sigma_yx": round(frames[t][j].sigma_yx, 3)},
            }
            parent = parents.get(cid)
            if parent is not None:
                record["parent"] = {
                    "id": parent,
                    "confidence": confidence(parent, cid),
                }
            if len(kids) == 2:
                record["state"] = "dividing"
            elif t == 1 and j == 5:
                record["state"] = "death"
            else:
                record["state"] = "normal"
            if j == 0:
                record["labels"] = ["ESI", "treated"]
            elif j == 3:
                record["labels"] = ["control"]
            # track-scope tag on the pre-division chain (cells 1 and 7)
            if j == 0 and t <= 1:
                record["track_labels"] = ["cell type 1"]
            records.append(record)
    return records


def tracking_document(cells: list[dict[str, Any]], source: str) -> dict[str, Any]:
    return {
        "format": "cellstudio-tracking",
        "version": 1,
        "metadata": {
            "created": "2026-08-21T00:00:00Z",
            "source": source,
            "shape_tczyx": [TINY_T, TINY_C, TINY_Z, TINY_Y, TINY_X],
            # the full vocabulary, including a name no cell carries
            "label_definitions": ["ESI", "cell type 1", "control", "treated", "unused"],
        },
        "cells": cells,
    }


def write_tracking(path: Path, doc: dict[str, Any], *, gzipped: bool = False) -> None:
    payload = json.dumps(doc, indent=1).encode()
    path.write_bytes(gzip.compress(payload, mtime=0) if gzipped else payload)


def find_cell(cells: list[dict[str, Any]], cell_id: int) -> dict[str, Any]:
    return next(c for c in cells if c["id"] == cell_id)


def build_tracking_valid(out: Path) -> str:
    cells = tracking_graph()
    doc = tracking_document(cells, "make_data.py: tiny scene, hand-built lineage")
    write_tracking(out / "tracks.json", doc)
    write_tracking(out / "tracks.json.gz", doc, gzipped=True)
    divisions = sum(1 for c in cells if len(c["children"]) == 2)
    return (
        f"Valid cellstudio-tracking v1 as .json and .json.gz (identical content): "
        f"{len(cells)} cells over {TINY_T} frames, {divisions} two-child division, "
        f"parent+children with confidences, state, user labels, passthrough features; "
        f"seg_ids resolve in labels_background0 and centroids/areas match it."
    )


def build_tracking_ids_too_wide(out: Path) -> str:
    cells = tracking_graph()
    leaf = TINY_BLOBS * 3 + 5
    wide = 2**32
    record = find_cell(cells, leaf)
    for entry in find_cell(cells, record["parent"]["id"])["children"]:
        if entry["id"] == leaf:
            entry["id"] = wide
    record["id"] = wide
    write_tracking(
        out / "tracks.json",
        tracking_document(cells, f"one cell id set to {wide} (2^32)"),
    )
    return (
        f"Tracking JSON whose only defect is a cell id of {wide} (2^32, one past "
        f"uint32 max {UINT32_MAX}), referenced consistently: the importer must reject "
        f"it and write nothing."
    )


def build_tracking_broken_reference(out: Path) -> str:
    cells = tracking_graph()
    leaf = TINY_BLOBS * 3 + 1
    find_cell(cells, leaf)["children"] = [{"id": 999_999, "confidence": 0.5}]
    write_tracking(
        out / "tracks.json",
        tracking_document(cells, "one children entry points at a nonexistent id"),
    )
    return (
        f"Tracking JSON whose only defect is cell {leaf} listing child 999999, which "
        f"has no record: import must abort naming the offending id and leave the "
        f"database unchanged."
    )


def build_tracking_child_earlier_frame(out: Path) -> str:
    cells = tracking_graph()
    parent = TINY_BLOBS * 3 + 2
    child = 3
    find_cell(cells, parent)["children"] = [{"id": child, "confidence": 0.6}]
    find_cell(cells, child)["parent"] = {"id": parent, "confidence": 0.6}
    write_tracking(
        out / "tracks.json",
        tracking_document(cells, "one link runs backwards in time"),
    )
    return (
        f"Tracking JSON whose only defect is a link from cell {parent} (t=3) to cell "
        f"{child} (t=0), mutually declared, so parent and children agree and only the "
        f"strictly-increasing-frame rule is violated: import must abort."
    )


BUILDERS: dict[str, Callable[[Path], str]] = {
    "tiny_v2": lambda out: build_tiny(out, 2),
    "tiny_v3": lambda out: build_tiny(out, 3),
    "no_scale_metadata": build_no_scale_metadata,
    "hostile_zbrick": lambda out: build_hostile(
        out, chunk_z=HOSTILE_Z, tile=HOSTILE_X // 2, kind="hostile_zbrick"
    ),
    "hostile_planes": lambda out: build_hostile(
        out, chunk_z=1, tile=HOSTILE_X, kind="hostile_planes"
    ),
    "labels_background0": lambda out: build_labels(out, reuse=False),
    "labels_reused_ids": lambda out: build_labels(out, reuse=True),
    "tracking_valid": build_tracking_valid,
    "tracking_ids_too_wide": build_tracking_ids_too_wide,
    "tracking_broken_reference": build_tracking_broken_reference,
    "tracking_child_earlier_frame": build_tracking_child_earlier_frame,
}


def dir_bytes(path: Path) -> int:
    if path.is_file():
        return path.stat().st_size
    return sum(p.stat().st_size for p in path.rglob("*") if p.is_file())


def zarr_stores(root: Path) -> Iterator[Path]:
    for name in ("image.zarr", "labels.zarr"):
        if (root / name).exists():
            yield root / name


def verify(out: Path, names: Sequence[str]) -> None:
    """Reopen every store and print what it actually says."""
    for name in names:
        base = out / name
        print(f"\n{name}  ({dir_bytes(base) / 1024:.1f} KiB)")
        for store in zarr_stores(base):
            group = zarr.open_group(store, mode="r")
            attrs = dict(group.attrs)
            ome = attrs.get("ome", attrs)
            entry = ome["multiscales"][0]
            axes = [a["name"] for a in entry["axes"]]
            print(
                f"  {store.name}: zarr_format={group.metadata.zarr_format} "
                f"axes={''.join(axes)} levels={len(entry['datasets'])} "
                f"ngff={ome.get('version', entry.get('version'))} "
                f"omero_channels="
                f"{len(ome.get('omero', {}).get('channels', [])) or None}"
            )
            for dataset in entry["datasets"]:
                array = group[dataset["path"]]
                scale = dataset.get("coordinateTransformations")
                zyx = dict(zip(axes, array.shape, strict=True))
                czyx = dict(zip(axes, array.chunks, strict=True))
                amp = read_amplification(
                    (zyx["z"], zyx["y"], zyx["x"]), (czyx["z"], czyx["y"], czyx["x"])
                )
                assert array.dtype in (np.uint16, np.uint32), array.dtype
                print(
                    f"    L{dataset['path']}: shape={array.shape} chunks={array.chunks} "
                    f"dtype={array.dtype} codec="
                    f"{_codec_name(array)} "
                    f"amp(xy/xz/yz)={amp['xy']:.0f}/{amp['xz']:.0f}/{amp['yz']:.0f}x "
                    f"scale={scale[0]['scale'] if scale else 'MISSING'}"
                )
            if "image-label" in attrs:
                data = group["0"][...]
                values = np.unique(data)
                print(
                    f"    labels: background0={0 in values} "
                    f"nonzero_values={values.size - 1} "
                    f"per_frame={[int(np.unique(data[t]).size - 1) for t in range(data.shape[0])]} "
                    f"max_id={int(values.max())}"
                )
        for track in sorted(base.glob("tracks.json*")):
            doc = json.loads(
                gzip.decompress(track.read_bytes())
                if track.suffix == ".gz"
                else track.read_bytes()
            )
            cells = doc["cells"]
            ids = [c["id"] for c in cells]
            links = sum(len(c.get("children", [])) for c in cells)
            print(
                f"  {track.name}: format={doc['format']} v{doc['version']} "
                f"cells={len(cells)} links={links} "
                f"divisions={sum(1 for c in cells if len(c.get('children', [])) == 2)} "
                f"max_id={max(ids)} fits_u32={max(ids) <= UINT32_MAX} "
                f"states={sorted({c['state'] for c in cells if 'state' in c})} "
                f"bytes={track.stat().st_size}"
            )


def cross_checks(out: Path) -> None:
    """Assert the couplings the stores advertise; skip pairs that weren't built."""

    def level0(name: str, store: str) -> np.ndarray:
        return zarr.open_group(out / name / store, mode="r")["0"][...]

    def have(*names: str) -> bool:
        return all((out / n).exists() for n in names)

    print("\ncross-data invariants")
    if have("tiny_v2", "tiny_v3"):
        for level in ("0", "1", "2"):
            v2 = zarr.open_group(out / "tiny_v2" / "image.zarr", mode="r")[level][...]
            v3 = zarr.open_group(out / "tiny_v3" / "image.zarr", mode="r")[level][...]
            assert np.array_equal(v2, v3), f"tiny_v2 != tiny_v3 at level {level}"
        print("  tiny_v2 == tiny_v3 voxel-for-voxel at all 3 levels")
    if have("hostile_zbrick", "hostile_planes"):
        a = level0("hostile_zbrick", "image.zarr")
        b = level0("hostile_planes", "image.zarr")
        assert np.array_equal(a, b), "hostile stores differ in content"
        print(
            "  hostile_zbrick == hostile_planes voxel-for-voxel (chunking is the only difference)"
        )
    if have("labels_background0", "labels_reused_ids"):
        unique = level0("labels_background0", "labels.zarr")
        reused = level0("labels_reused_ids", "labels.zarr")
        assert np.array_equal(unique > 0, reused > 0), "label geometry differs"
        print(
            "  labels_background0 and labels_reused_ids share the same foreground geometry"
        )
    if have("tracking_valid"):
        raw = (out / "tracking_valid" / "tracks.json").read_bytes()
        gz = gzip.decompress((out / "tracking_valid" / "tracks.json.gz").read_bytes())
        assert raw == gz, "tracks.json.gz decompresses to different bytes"
        print(
            "  tracking_valid tracks.json.gz decompresses byte-identically to tracks.json"
        )
    if have("tracking_valid", "labels_background0", "tiny_v2"):
        cells = json.loads(raw)["cells"]
        labels = level0("labels_background0", "labels.zarr")
        image = zarr.open_group(out / "tiny_v2" / "image.zarr", mode="r")["0"]
        assert list(json.loads(raw)["metadata"]["shape_tczyx"]) == list(image.shape)
        for cell in cells:
            frame = labels[cell["t"]]
            assert cell["seg_id"] in np.unique(frame), f"seg_id {cell['seg_id']} absent"
            centroid, area = region_stats(frame, cell["seg_id"])
            assert centroid == cell["centroid"], f"centroid mismatch for {cell['id']}"
            assert area == cell["features"]["area"], f"area mismatch for {cell['id']}"
        print(
            f"  all {len(cells)} tracking_valid seg_ids resolve in labels_background0; "
            f"centroids and areas match the masks; shape_tczyx == tiny_v2 shape"
        )


def _codec_name(array: zarr.Array) -> str:
    codecs = getattr(array, "compressors", None) or ()
    return (
        ",".join(
            f"{getattr(c, 'cname', type(c).__name__)}"
            f"{f':{c.clevel}' if hasattr(c, 'clevel') else ''}"
            for c in codecs
        )
        or "none"
    )


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--out",
        type=Path,
        default=Path(".data"),
        help="Output dir (default .data)",
    )
    parser.add_argument(
        "--only",
        action="append",
        choices=sorted(BUILDERS),
        help="Generate only this store (repeatable)",
    )
    parser.add_argument(
        "--verify-only", action="store_true", help="Re-verify without regenerating"
    )
    parser.add_argument("--list", action="store_true", help="List store names and exit")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> None:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    if args.list:
        for name in sorted(BUILDERS):
            print(name)
        return

    out = args.out.expanduser().resolve()
    names = args.only or sorted(BUILDERS)

    if not args.verify_only:
        out.mkdir(parents=True, exist_ok=True)
        index: dict[str, Any] = {}
        index_path = out / "manifest.json"
        if index_path.is_file():
            index = json.loads(index_path.read_text())
        for name in names:
            base = out / name
            if base.exists():
                if not (base / "MANIFEST").is_file():
                    raise SystemExit(
                        f"{base} exists and is not a store dir (no MANIFEST); "
                        f"move it aside"
                    )
                shutil.rmtree(base)
            base.mkdir(parents=True)
            note = BUILDERS[name](base)
            (base / "MANIFEST").write_text(note + "\n")
            artifacts = sorted(p.name for p in base.iterdir() if p.name != "MANIFEST")
            index[name] = {
                "path": name,
                "artifacts": artifacts,
                "bytes": dir_bytes(base),
                "note": note,
            }
            print(f"{name:28} {dir_bytes(base) / 1024:8.1f} KiB  {artifacts}")
        index_path.write_text(json.dumps(dict(sorted(index.items())), indent=2) + "\n")
        total = sum(dir_bytes(out / n) for n in sorted(BUILDERS) if (out / n).exists())
        print(
            f"\n{len(names)} stores written to {out}; set total {total / 1024:.1f} KiB"
        )

    verify(out, names)
    cross_checks(out)


if __name__ == "__main__":
    main()
