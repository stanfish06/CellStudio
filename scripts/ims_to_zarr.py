#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "h5py",
#     "numcodecs",
#     "numpy",
#     "zarr>=3,<4",
# ]
# ///
"""Convert an Imaris .ims to OME-NGFF 0.4 / Zarr v2.

Example:
    uv run scripts/ims_to_zarr.py 260817_EXP63_live_bse_fa100_F00.ims
"""

from __future__ import annotations

import argparse
import json
import shutil
import sys
from datetime import datetime
from itertools import pairwise
from pathlib import Path
from typing import Any

import h5py
import numpy as np
import zarr
from numcodecs import Blosc

AXES = ("t", "c", "z", "y", "x")


def decode_imaris_attr(value: Any) -> str:
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace").strip("\x00").strip()
    if isinstance(value, np.ndarray):
        if value.dtype.kind in {"S", "O"} and value.ndim == 1:
            parts = [v if isinstance(v, bytes) else bytes([v]) for v in value.tolist()]
            return (
                b"".join(parts).decode("utf-8", errors="replace").strip("\x00").strip()
            )
        if value.shape == () or value.size == 1:
            return decode_imaris_attr(value.reshape(()).item())
    return str(value).strip()


def group_attrs(group: h5py.Group) -> dict[str, str]:
    return {str(k): decode_imaris_attr(v) for k, v in group.attrs.items()}


def parse_color(text: str) -> tuple[float, float, float]:
    parts = [float(p) for p in text.split()]
    while len(parts) < 3:
        parts.append(0.0)
    return parts[0], parts[1], parts[2]


def rgb_to_hex(rgb: tuple[float, float, float]) -> str:
    return "".join(f"{max(0, min(255, round(c * 255))):02X}" for c in rgb)


def parse_timestamp(text: str) -> datetime | None:
    cleaned = text.strip()
    if not cleaned:
        return None
    try:
        return datetime.fromisoformat(cleaned)
    except ValueError:
        return None


def sorted_index_names(group: h5py.Group, prefix: str) -> list[int]:
    out: list[int] = []
    for name in group:
        if name.startswith(prefix):
            out.append(int(name.split()[-1]))
    return sorted(out)


def downsample_yx(volume: np.ndarray) -> np.ndarray:
    """Mean-pool ZYX by 2 in Y and X. Drops a trailing odd row/col."""
    z, y, x = volume.shape
    y2, x2 = y // 2, x // 2
    if y2 == 0 or x2 == 0:
        raise ValueError(f"cannot downsample shape {volume.shape}")
    cropped = volume[:, : y2 * 2, : x2 * 2]
    pooled = cropped.reshape(z, y2, 2, x2, 2).mean(axis=(2, 4))
    return np.rint(pooled).astype(volume.dtype, copy=False)


def choose_z_size(dataset: h5py.Dataset, declared_z: int | None) -> int:
    stored_z = int(dataset.shape[0])
    if declared_z is None or declared_z <= 0 or declared_z > stored_z:
        return stored_z
    if declared_z == stored_z:
        return stored_z
    trailing = dataset[declared_z:]
    if trailing.size == 0 or np.any(trailing):
        return stored_z
    return declared_z


def voxel_sizes_um(
    image_info: dict[str, str], shape_zyx: tuple[int, int, int]
) -> tuple[float, float, float]:
    z, y, x = shape_zyx

    def extent(axis: int) -> float | None:
        lo = image_info.get(f"ExtMin{axis}")
        hi = image_info.get(f"ExtMax{axis}")
        if lo is None or hi is None:
            return None
        return abs(float(hi) - float(lo))

    dx = extent(0)
    dy = extent(1)
    dz = extent(2)
    return (
        (dz / z) if dz and z else 1.0,
        (dy / y) if dy and y else 1.0,
        (dx / x) if dx and x else 1.0,
    )


def time_scale_seconds(timestamps: list[str]) -> float:
    parsed = [parse_timestamp(t) for t in timestamps]
    deltas: list[float] = []
    for a, b in pairwise(parsed):
        if a is None or b is None:
            continue
        dt = (b - a).total_seconds()
        if dt > 0:
            deltas.append(dt)
    if not deltas:
        return 1.0
    return float(np.median(np.asarray(deltas, dtype=np.float64)))


def chunk_shape(shape: tuple[int, ...], tile: int) -> tuple[int, ...]:
    _, _, z, y, x = shape
    return (1, 1, z, min(tile, y), min(tile, x))


def inspect_ims(ims_path: Path) -> None:
    with h5py.File(ims_path, "r") as handle:
        dataset = handle["DataSet"]
        levels = sorted_index_names(dataset, "ResolutionLevel ")
        image_info = group_attrs(handle["DataSetInfo/Image"])
        declared_z = int(image_info["Z"]) if "Z" in image_info else None
        print(f"file: {ims_path}")
        print(f"resolution levels: {levels}")
        for level in levels:
            level_group = dataset[f"ResolutionLevel {level}"]
            times = sorted_index_names(level_group, "TimePoint ")
            channels = sorted_index_names(
                level_group[f"TimePoint {times[0]}"], "Channel "
            )
            sample = level_group[f"TimePoint {times[0]}/Channel {channels[0]}/Data"]
            z = choose_z_size(sample, declared_z)
            print(
                f"  L{level}: T={len(times)} C={len(channels)} "
                f"stored_ZYX={sample.shape} used_Z={z} dtype={sample.dtype} "
                f"chunks={sample.chunks}"
            )
        print("image:")
        for key in ("Name", "X", "Y", "Z", "Unit", "RecordingDate", "MicroscopeMode"):
            if key in image_info:
                print(f"  {key}: {image_info[key]!r}")
        info = handle["DataSetInfo"]
        for name in sorted(k for k in info if k.startswith("Channel ")):
            attrs = group_attrs(info[name])
            print(
                f"  {name}: {attrs.get('Name', '')!r} color={attrs.get('Color')!r} "
                f"range={attrs.get('ColorRange')!r}"
            )


def build_omero(
    name: str,
    channels: list[dict[str, str]],
    dtype: np.dtype[Any],
    default_z: int,
) -> dict[str, Any]:
    info_max = float(np.iinfo(dtype).max) if np.issubdtype(dtype, np.integer) else 1.0
    omero_channels: list[dict[str, Any]] = []
    for index, attrs in enumerate(channels):
        rgb = parse_color(attrs.get("Color", "1 1 1"))
        window = attrs.get("ColorRange", "0 1").split()
        start = float(window[0]) if window else 0.0
        end = float(window[1]) if len(window) > 1 else info_max
        omero_channels.append(
            {
                "active": True,
                "coefficient": 1.0,
                "color": rgb_to_hex(rgb),
                "family": "linear",
                "inverted": False,
                "label": attrs.get("Name", f"Channel {index}"),
                "window": {
                    "min": 0.0,
                    "max": info_max,
                    "start": start,
                    "end": end,
                },
            }
        )
    return {
        "id": 1,
        "name": name,
        "version": "0.4",
        "channels": omero_channels,
        "rdefs": {"defaultT": 0, "defaultZ": default_z, "model": "color"},
    }


def write_multiscales(
    root: zarr.Group,
    *,
    name: str,
    level_shapes: list[tuple[int, int, int, int, int]],
    dt_s: float,
    voxel_zyx: tuple[float, float, float],
    omero: dict[str, Any],
    extra_attrs: dict[str, Any],
) -> None:
    dz, dy, dx = voxel_zyx
    datasets: list[dict[str, Any]] = []
    for level, _shape in enumerate(level_shapes):
        factor = 2**level
        datasets.append(
            {
                "path": str(level),
                "coordinateTransformations": [
                    {
                        "type": "scale",
                        "scale": [dt_s, 1.0, dz, dy * factor, dx * factor],
                    }
                ],
            }
        )
    root.attrs.update(
        {
            "multiscales": [
                {
                    "version": "0.4",
                    "name": name,
                    "axes": [
                        {"name": "t", "type": "time", "unit": "second"},
                        {"name": "c", "type": "channel"},
                        {"name": "z", "type": "space", "unit": "micrometer"},
                        {"name": "y", "type": "space", "unit": "micrometer"},
                        {"name": "x", "type": "space", "unit": "micrometer"},
                    ],
                    "datasets": datasets,
                    "type": "local_mean",
                    "metadata": {"method": "local_mean", "version": "0.4"},
                }
            ],
            "omero": omero,
            **extra_attrs,
        }
    )


def convert(
    ims_path: Path,
    out_path: Path,
    *,
    max_t: int | None,
    tile: int,
    overwrite: bool,
) -> None:
    if out_path.exists():
        if not overwrite:
            raise SystemExit(f"refusing to overwrite {out_path} (pass --overwrite)")
        shutil.rmtree(out_path)

    out_path.parent.mkdir(parents=True, exist_ok=True)

    with h5py.File(ims_path, "r") as handle:
        level0 = handle["DataSet/ResolutionLevel 0"]
        time_ids = sorted_index_names(level0, "TimePoint ")
        if max_t is not None:
            time_ids = time_ids[:max_t]
        channel_ids = sorted_index_names(level0[f"TimePoint {time_ids[0]}"], "Channel ")
        sample = level0[f"TimePoint {time_ids[0]}/Channel {channel_ids[0]}/Data"]
        image_info = group_attrs(handle["DataSetInfo/Image"])
        declared_z = int(image_info["Z"]) if "Z" in image_info else None
        z_size = choose_z_size(sample, declared_z)
        y_size, x_size = int(sample.shape[1]), int(sample.shape[2])
        dtype = np.dtype(sample.dtype)
        if dtype != np.uint16 and dtype != np.uint8:
            raise SystemExit(
                f"unsupported sample dtype {dtype}; expected uint8 or uint16"
            )

        t_count, c_count = len(time_ids), len(channel_ids)
        shape0 = (t_count, c_count, z_size, y_size, x_size)
        voxel_zyx = voxel_sizes_um(image_info, (z_size, y_size, x_size))

        channel_attrs = [
            group_attrs(handle["DataSetInfo"][f"Channel {c}"])
            if f"Channel {c}" in handle["DataSetInfo"]
            else {}
            for c in channel_ids
        ]
        time_info = group_attrs(handle["DataSetInfo/TimeInfo"])
        timestamps = [time_info.get(f"TimePoint{i + 1}", "") for i in range(t_count)]
        dt_s = time_scale_seconds(timestamps)
        name = Path(image_info.get("Name", ims_path.stem)).stem or ims_path.stem

        level_shapes: list[tuple[int, int, int, int, int]] = [shape0]
        y, x = y_size, x_size
        while min(y, x) > 256:
            y, x = y // 2, x // 2
            level_shapes.append((t_count, c_count, z_size, y, x))

        nbytes = sum(int(np.prod(s)) * dtype.itemsize for s in level_shapes)
        print(
            f"writing {out_path}\n"
            f"  shape0 TCZYX={shape0} dtype={dtype} levels={len(level_shapes)}\n"
            f"  voxel um z,y,x={voxel_zyx} dt={dt_s:.3f}s\n"
            f"  uncompressed pyramid={nbytes / 1e9:.2f} GB"
        )

        compressor = Blosc(cname="zstd", clevel=5, shuffle=Blosc.SHUFFLE)
        root = zarr.create_group(out_path, overwrite=True, zarr_format=2)
        arrays: list[zarr.Array] = []
        for level, shape in enumerate(level_shapes):
            array = root.create_array(
                str(level),
                shape=shape,
                chunks=chunk_shape(shape, tile),
                dtype=dtype,
                compressors=compressor,
                fill_value=0,
            )
            array.attrs["_ARRAY_DIMENSIONS"] = list(AXES)
            arrays.append(array)

        total = t_count * c_count
        done = 0
        for t_index, t_id in enumerate(time_ids):
            for c_index, c_id in enumerate(channel_ids):
                source = level0[f"TimePoint {t_id}/Channel {c_id}/Data"]
                volume = np.asarray(source[:z_size], dtype=dtype)
                arrays[0][t_index, c_index] = volume
                current = volume
                for array in arrays[1:]:
                    current = downsample_yx(current)
                    array[t_index, c_index] = current
                done += 1
                if done == 1 or done == total or done % 10 == 0:
                    print(f"  {done}/{total}  t={t_index} c={c_index}", flush=True)

        extra = {
            "source_ims": ims_path.name,
            "imaris_name": image_info.get("Name", ""),
            "recording_date": image_info.get("RecordingDate", ""),
            "time_stamps": timestamps,
            "channel_excitation_nm": [
                attrs.get("LSMExcitationWavelength", "") for attrs in channel_attrs
            ],
            "channel_emission_nm": [
                attrs.get("LSMEmissionWavelength", "") for attrs in channel_attrs
            ],
        }
        write_multiscales(
            root,
            name=name,
            level_shapes=level_shapes,
            dt_s=dt_s,
            voxel_zyx=voxel_zyx,
            omero=build_omero(name, channel_attrs, dtype, default_z=z_size // 2),
            extra_attrs=extra,
        )

    stored = sum(p.stat().st_size for p in out_path.rglob("*") if p.is_file())
    print(f"done  {out_path}  {stored / 1e9:.2f} GB on disk")
    print(
        json.dumps(
            {"shape": list(shape0), "levels": len(level_shapes), "dt_s": dt_s}, indent=2
        )
    )


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("ims", type=Path, help="Imaris .ims file")
    parser.add_argument(
        "-o",
        "--output",
        type=Path,
        default=None,
        help="Output .zarr directory (default: .data/<stem>.zarr)",
    )
    parser.add_argument(
        "--inspect", action="store_true", help="Print IMS layout and exit"
    )
    parser.add_argument(
        "--max-t", type=int, default=None, help="Convert only the first N timepoints"
    )
    parser.add_argument(
        "--tile", type=int, default=1024, help="Max Y/X chunk size (default 1024)"
    )
    parser.add_argument(
        "--overwrite", action="store_true", help="Replace an existing output store"
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> None:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    ims_path = args.ims.expanduser().resolve()
    if not ims_path.is_file():
        raise SystemExit(f"not a file: {ims_path}")
    if args.inspect:
        inspect_ims(ims_path)
        return
    out_path = args.output
    if out_path is None:
        out_path = Path(".data") / f"{ims_path.stem}.zarr"
    convert(
        ims_path,
        out_path.expanduser().resolve(),
        max_t=args.max_t,
        tile=args.tile,
        overwrite=args.overwrite,
    )


if __name__ == "__main__":
    main()
