#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "numcodecs",
#     "numpy",
#     "zarr>=3,<4",
# ]
# ///
"""Generate the synthetic bench store: deep and wide OME-Zarr for budget runs.

Example:
    uv run scripts/make_bench_data.py --out data/bench.zarr
    uv run scripts/make_bench_data.py --out data/bench.zarr --dry-run
    uv run scripts/make_bench_data.py --out /vol/bench.zarr --scale 2 --max-disk-gb 40
"""

from __future__ import annotations

import argparse
import json
import math
import shutil
import sys
import time
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

import numpy as np
import zarr
from numcodecs import Blosc

AXES = ("t", "c", "z", "y", "x")
GIB = 1024**3

BLOB_PITCH_ZYX = (10, 56, 56)
BLOB_SIGMA_Z = 1.6
BLOB_SIGMA_YX = 4.5
BLOB_KEEP = 0.8
BLOB_SEED = 20260821
NOISE_SEED = 99

VOXEL_ZYX = (2.0, 0.6, 0.6)
DT_S = 600.0
BASELINE = 110
CHANNEL_SIGNAL = (6000.0, 2400.0)
CHANNEL_SIGMA_GAIN = (1.0, 1.7)
CHANNEL_META = (
    ("H2B-mNeonGreen", "37FF00", (140, 5400)),
    ("CAAX-mScarlet", "FF0000", (140, 2300)),
)


def level_shapes(
    shape: tuple[int, int, int, int, int], min_level: int
) -> list[tuple[int, int, int, int, int]]:
    """XY-only pyramid, halving until min(y, x) <= min_level (as ims_to_zarr.py)."""
    t, c, z, y, x = shape
    shapes = [shape]
    while min(y, x) > min_level:
        y, x = y // 2, x // 2
        shapes.append((t, c, z, y, x))
    return shapes


def raw_bytes(shapes: list[tuple[int, ...]], itemsize: int) -> int:
    return sum(int(np.prod(s, dtype=np.int64)) * itemsize for s in shapes)


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


def check_budget(projected: int, max_disk_gb: float, out: Path) -> None:
    """Refuse before writing: the projection is raw bytes, an upper bound."""
    budget = int(max_disk_gb * GIB)
    anchor = next(p for p in [out, *out.parents] if p.exists())
    free = shutil.disk_usage(anchor).free
    print(
        f"  budget       {max_disk_gb:.3f} GiB (--max-disk-gb)\n"
        f"  free on disk {free / GIB:.3f} GiB at {anchor}"
    )
    if projected > budget:
        raise SystemExit(
            f"refusing to start: projected raw footprint {projected / GIB:.3f} GiB "
            f"exceeds --max-disk-gb {max_disk_gb:.3f} GiB. Lower --scale/--t/--z, or "
            f"raise --max-disk-gb if the disk really has room (blosc-zstd will store "
            f"less than the raw projection, but the guard does not gamble on it)."
        )
    if projected > free:
        raise SystemExit(
            f"refusing to start: projected raw footprint {projected / GIB:.3f} GiB "
            f"exceeds {free / GIB:.3f} GiB free at {anchor}"
        )


def axis_centers(n: int, pitch: int) -> tuple[np.ndarray, float]:
    """Evenly spaced centers at ~`pitch` spacing, never fewer than one."""
    count = max(1, int(n / pitch + 0.5))
    step = n / count
    return np.arange(count, dtype=np.float64) * step + step / 2, step


def blob_grid(
    shape_zyx: tuple[int, int, int], seed: int
) -> tuple[np.ndarray, np.ndarray]:
    """Jittered-grid nucleus centers [n,3] (z,y,x) with per-nucleus amplitude."""
    rng = np.random.default_rng(seed)
    grids, steps = zip(
        *(axis_centers(n, p) for n, p in zip(shape_zyx, BLOB_PITCH_ZYX, strict=True)),
        strict=True,
    )
    centers = np.stack(np.meshgrid(*grids, indexing="ij"), axis=-1).reshape(-1, 3)
    centers += rng.uniform(-0.35, 0.35, size=centers.shape) * np.asarray(steps)
    centers = np.clip(centers, 0.5, np.asarray(shape_zyx, dtype=np.float64) - 1.5)
    keep = rng.random(len(centers)) < BLOB_KEEP
    centers = centers[keep]
    amps = 0.45 + 0.55 * rng.random(len(centers)).astype(np.float32)
    return centers.astype(np.float32), amps


def drift(centers: np.ndarray, frame: int) -> np.ndarray:
    """Per-frame random walk so nuclei move between timepoints."""
    rng = np.random.default_rng(BLOB_SEED + frame)
    step = rng.normal(0.0, 1.0, size=centers.shape).astype(np.float32)
    step[:, 0] *= 0.25
    return centers + step * frame


def render_slab(
    centers: np.ndarray,
    amps: np.ndarray,
    z0: int,
    z1: int,
    shape_zyx: tuple[int, int, int],
    sigma_gain: float,
) -> np.ndarray:
    """Sum of separable gaussians over z in [z0, z1), one local window per blob."""
    _, y, x = shape_zyx
    sz = BLOB_SIGMA_Z * sigma_gain
    sxy = BLOB_SIGMA_YX * sigma_gain
    rz, rxy = math.ceil(3 * sz), math.ceil(3 * sxy)
    field = np.zeros((z1 - z0, y, x), dtype=np.float32)
    near = (centers[:, 0] > z0 - rz - 1) & (centers[:, 0] < z1 + rz + 1)
    for (cz, cy, cx), amp in zip(centers[near], amps[near], strict=True):
        za, zb = max(z0, int(cz) - rz), min(z1, int(cz) + rz + 1)
        ya, yb = max(0, int(cy) - rxy), min(y, int(cy) + rxy + 1)
        xa, xb = max(0, int(cx) - rxy), min(x, int(cx) + rxy + 1)
        if za >= zb or ya >= yb or xa >= xb:
            continue
        gz = np.exp(-((np.arange(za, zb, dtype=np.float32) - cz) ** 2) / (2 * sz * sz))
        gy = np.exp(
            -((np.arange(ya, yb, dtype=np.float32) - cy) ** 2) / (2 * sxy * sxy)
        )
        gx = np.exp(
            -((np.arange(xa, xb, dtype=np.float32) - cx) ** 2) / (2 * sxy * sxy)
        )
        field[za - z0 : zb - z0, ya:yb, xa:xb] += amp * (
            gz[:, None, None] * gy[None, :, None] * gx[None, None, :]
        )
    return field


def quantize(field: np.ndarray, rng: np.random.Generator, signal: float) -> np.ndarray:
    """Illumination gradient + shot noise per plane, cast to uint16."""
    _, y, x = field.shape
    ramp = (
        np.linspace(0.0, 1.0, y, dtype=np.float32)[:, None]
        * np.linspace(0.2, 1.0, x, dtype=np.float32)[None, :]
    )
    out = np.empty(field.shape, dtype=np.uint16)
    for i in range(field.shape[0]):
        counts = BASELINE + 0.06 * signal * ramp + signal * field[i]
        counts += rng.standard_normal(counts.shape, dtype=np.float32) * np.sqrt(counts)
        np.clip(counts, 0, 65535, out=counts)
        out[i] = counts.astype(np.uint16)
    return out


def downsample_yx(volume: np.ndarray) -> np.ndarray:
    """Mean-pool ZYX by 2 in Y and X. Drops a trailing odd row/col."""
    z, y, x = volume.shape
    y2, x2 = y // 2, x // 2
    if y2 == 0 or x2 == 0:
        raise ValueError(f"cannot downsample shape {volume.shape}")
    pooled = volume[:, : y2 * 2, : x2 * 2].reshape(z, y2, 2, x2, 2).mean(axis=(2, 4))
    return np.rint(pooled).astype(volume.dtype, copy=False)


def multiscale_attrs(
    shapes: list[tuple[int, int, int, int, int]], n_channels: int, z: int
) -> dict[str, Any]:
    dz, dy, dx = VOXEL_ZYX
    start = datetime(2026, 8, 21, 9, 0, 0, tzinfo=UTC)
    return {
        "multiscales": [
            {
                "version": "0.4",
                "name": "bench",
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
                        "coordinateTransformations": [
                            {
                                "type": "scale",
                                "scale": [
                                    DT_S,
                                    1.0,
                                    dz,
                                    dy * 2**level,
                                    dx * 2**level,
                                ],
                            }
                        ],
                    }
                    for level in range(len(shapes))
                ],
                "type": "local_mean",
                "metadata": {"method": "local_mean", "version": "0.4"},
            }
        ],
        "omero": {
            "id": 1,
            "name": "bench",
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
                        "start": float(start_v),
                        "end": float(end_v),
                    },
                }
                for label, color, (start_v, end_v) in CHANNEL_META[:n_channels]
            ],
            "rdefs": {"defaultT": 0, "defaultZ": z // 2, "model": "color"},
        },
        "time_stamps": [
            (start + timedelta(seconds=DT_S * i)).isoformat(
                sep=" ", timespec="milliseconds"
            )
            for i in range(shapes[0][0])
        ],
        "synthetic": {
            "generator": "scripts/make_bench_data.py",
            "blob_pitch_zyx": list(BLOB_PITCH_ZYX),
            "blob_sigma_z_yx": [BLOB_SIGMA_Z, BLOB_SIGMA_YX],
            "seed": BLOB_SEED,
        },
    }


def generate(
    out: Path,
    *,
    shape: tuple[int, int, int, int, int],
    min_level: int,
    chunk_z: int,
    chunk_xy: int,
    overwrite: bool,
) -> None:
    t_count, c_count, z, y, x = shape
    shapes = level_shapes(shape, min_level)
    if out.exists():
        if not overwrite:
            raise SystemExit(f"refusing to overwrite {out} (pass --overwrite)")
        shutil.rmtree(out)
    out.parent.mkdir(parents=True, exist_ok=True)

    compressor = Blosc(cname="zstd", clevel=5, shuffle=Blosc.SHUFFLE)
    root = zarr.create_group(out, overwrite=True, zarr_format=2)
    arrays: list[zarr.Array] = []
    for level, level_shape in enumerate(shapes):
        _, _, lz, ly, lx = level_shape
        array = root.create_array(
            str(level),
            shape=level_shape,
            chunks=(1, 1, min(chunk_z, lz), min(chunk_xy, ly), min(chunk_xy, lx)),
            dtype=np.uint16,
            compressors=compressor,
            fill_value=0,
        )
        array.attrs["_ARRAY_DIMENSIONS"] = list(AXES)
        arrays.append(array)

    centers, amps = blob_grid((z, y, x), BLOB_SEED)
    rng = np.random.default_rng(NOISE_SEED)
    slabs = list(range(0, z, chunk_z))
    total = t_count * c_count * len(slabs)
    done = 0
    started = time.monotonic()
    print(f"  nuclei per volume: {len(centers)}")
    for t in range(t_count):
        moved = drift(centers, t)
        for c in range(c_count):
            for z0 in slabs:
                z1 = min(z0 + chunk_z, z)
                field = render_slab(
                    moved, amps, z0, z1, (z, y, x), CHANNEL_SIGMA_GAIN[c % 2]
                )
                slab = quantize(field, rng, CHANNEL_SIGNAL[c % 2])
                del field
                arrays[0][t, c, z0:z1] = slab
                for array in arrays[1:]:
                    slab = downsample_yx(slab)
                    array[t, c, z0:z1] = slab
                done += 1
                if done == 1 or done == total or done % 8 == 0:
                    rate = (time.monotonic() - started) / done
                    print(
                        f"  {done}/{total}  t={t} c={c} z={z0}:{z1}  "
                        f"{rate:.2f}s/slab  eta {(total - done) * rate:.0f}s",
                        flush=True,
                    )

    root.attrs.update(multiscale_attrs(shapes, c_count, z))


def dir_bytes(path: Path) -> int:
    return sum(p.stat().st_size for p in path.rglob("*") if p.is_file())


def verify(out: Path, projected: int) -> None:
    group = zarr.open_group(out, mode="r")
    entry = dict(group.attrs)["multiscales"][0]
    print(f"\nverify {out}")
    print(
        f"  zarr_format={group.metadata.zarr_format} "
        f"ngff={entry['version']} levels={len(entry['datasets'])} "
        f"channels={len(dict(group.attrs)['omero']['channels'])}"
    )
    for dataset in entry["datasets"]:
        array = group[dataset["path"]]
        _, _, z, y, x = array.shape
        _, _, cz, cy, cx = array.chunks
        amp = read_amplification((z, y, x), (cz, cy, cx))
        sample = array[0, 0, z // 2]
        print(
            f"  L{dataset['path']}: shape={array.shape} chunks={array.chunks} "
            f"dtype={array.dtype} scale={dataset['coordinateTransformations'][0]['scale']}\n"
            f"       amp(xy/xz/yz)={amp['xy']:.0f}/{amp['xz']:.0f}/{amp['yz']:.0f}x  "
            f"mid-plane min/mean/max={sample.min()}/{sample.mean():.0f}/{sample.max()}"
        )
    stored = dir_bytes(out)
    print(
        f"  projected raw {projected / GIB:.3f} GiB  ->  on disk "
        f"{stored / GIB:.3f} GiB  (ratio {projected / max(stored, 1):.2f}x, "
        f"{sum(1 for _ in out.rglob('*') if _.is_file())} files)"
    )


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--out", type=Path, default=Path("data/bench.zarr"), help="Output .zarr dir"
    )
    parser.add_argument(
        "--max-disk-gb",
        type=float,
        default=8.0,
        help="Refuse if the raw pyramid exceeds this many GiB (default 8)",
    )
    parser.add_argument(
        "--scale", type=float, default=1.0, help="Multiply the XY plane (5.0 = 10k^2)"
    )
    parser.add_argument("--xy", type=int, default=2048, help="Base plane size")
    parser.add_argument("--z", type=int, default=45, help="Z planes")
    parser.add_argument("--t", type=int, default=8, help="Timepoints")
    parser.add_argument("--channels", type=int, default=2, help="Channels (1 or 2)")
    parser.add_argument("--chunk-z", type=int, default=16, help="Chunk z extent")
    parser.add_argument("--chunk-xy", type=int, default=256, help="Chunk y/x extent")
    parser.add_argument(
        "--min-level", type=int, default=256, help="Stop the pyramid at this plane size"
    )
    parser.add_argument(
        "--dry-run", action="store_true", help="Project the footprint and exit"
    )
    parser.add_argument(
        "--overwrite", action="store_true", help="Replace an existing output store"
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> None:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    if not 1 <= args.channels <= 2:
        raise SystemExit("--channels must be 1 or 2")
    plane = max(1, round(args.xy * args.scale))
    shape = (args.t, args.channels, args.z, plane, plane)
    shapes = level_shapes(shape, args.min_level)
    projected = raw_bytes(shapes, 2)
    out = args.out.expanduser().resolve()

    print(f"projection for {out}")
    print(f"  shape TCZYX={shape} uint16 levels={len(shapes)}")
    print(f"  level plane sizes: {[s[3] for s in shapes]}")
    print(
        f"  raw pyramid  {projected / GIB:.3f} GiB "
        f"(level 0 {raw_bytes(shapes[:1], 2) / GIB:.3f} GiB)"
    )
    check_budget(projected, args.max_disk_gb, out)
    if args.dry_run:
        print("  dry run: nothing written")
        return

    started = time.monotonic()
    generate(
        out,
        shape=shape,
        min_level=args.min_level,
        chunk_z=args.chunk_z,
        chunk_xy=args.chunk_xy,
        overwrite=args.overwrite,
    )
    verify(out, projected)
    print(f"  wall time {time.monotonic() - started:.1f}s")
    print(json.dumps({"path": str(out), "shape": list(shape), "levels": len(shapes)}))


if __name__ == "__main__":
    main()
