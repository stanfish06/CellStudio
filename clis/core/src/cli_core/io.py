import glob as globlib
from pathlib import Path

import numpy as np
import tifffile

TIFF_SUFFIXES = {".tif", ".tiff"}
# formats handled by the installed bioio reader plugins
# (ims/vsi/oir need the opt-in bioformats extra)
BIOIO_SUFFIXES = {
    ".tif",
    ".tiff",
    ".nd2",
    ".czi",
    ".lif",
    ".dv",
    ".r3d",
    ".png",
    ".jpg",
    ".jpeg",
    ".bmp",
    ".gif",
    ".zarr",
    ".ims",
    ".vsi",
    ".oir",
    ".lsm",
    ".stk",
}
STACKABLE_SUFFIXES = TIFF_SUFFIXES | BIOIO_SUFFIXES | {".npy"}


def read_image(
    path: Path, scene: int | str | None = None, channel: int | str | None = None
) -> np.ndarray:
    """Read an image/stack from most microscopy formats.

    tiff/npy are read raw; everything else goes through bioio (TCZYX, then
    singleton axes squeezed). A directory or a glob pattern (e.g. /data/*.ims)
    is read as sorted files stacked on axis 0. scene/channel select from
    multi-scene/multi-channel files and force the bioio path even for tiffs.
    """
    if globlib.has_magic(str(path)):
        matches = sorted(Path(p) for p in globlib.glob(str(path)))
        if not matches:
            raise FileNotFoundError(f"no files match pattern {path}")
        if len(matches) == 1:
            return read_image(matches[0], scene, channel)
        print(f"[io] pattern {path}: stacking {len(matches)} files on axis 0")
        return np.stack([read_image(p, scene, channel) for p in matches])
    path = Path(path)
    if not path.exists():
        raise FileNotFoundError(path)
    if path.suffix.lower() == ".zarr":
        return _read_bioio(path, scene, channel)
    if path.is_dir():
        files = sorted(
            p for p in path.iterdir() if p.suffix.lower() in STACKABLE_SUFFIXES
        )
        if not files:
            raise FileNotFoundError(f"no stackable image files in directory {path}")
        return np.stack([read_image(f, scene, channel) for f in files])
    suffix = path.suffix.lower()
    if suffix == ".npy":
        return np.load(path)
    if suffix in TIFF_SUFFIXES and scene is None and channel is None:
        return tifffile.imread(path)
    return _read_bioio(path, scene, channel)


def _read_bioio(
    path: Path, scene: int | str | None, channel: int | str | None
) -> np.ndarray:
    try:
        from bioio import BioImage
    except ImportError as e:
        raise RuntimeError("bioio is not installed in this environment") from e
    try:
        img = BioImage(path)
    except Exception as e:
        raise ValueError(
            f"no installed reader supports {path.name} ({e}); "
            "for vsi/ims/oir install the bioformats extra: cellstudio-cli-core[bioformats]"
        ) from e
    if scene is not None:
        img.set_scene(scene)
    data = img.data  # TCZYX
    full_shape = data.shape
    try:
        names = [str(n) for n in img.channel_names or []]
    except Exception:  # noqa: BLE001
        names = []
    if channel is not None:
        if isinstance(channel, str):
            if channel not in names:
                raise ValueError(f"channel '{channel}' not in {names}")
            channel = names.index(channel)
        data = data[:, channel : channel + 1]
    squeezed = np.squeeze(data)
    # axes surviving the squeeze, with their indices in the array fed to the model
    kept = [(ax, n) for ax, n in zip("TCZYX", data.shape) if n > 1]
    kept_axes = "".join(ax for ax, _ in kept)
    axis_map = ", ".join(f"{ax}=axis {i}" for i, (ax, _) in enumerate(kept))
    # metadata accessors can raise on files with sparse metadata; never fail a good read over the log line
    try:
        scene_label = img.current_scene
    except Exception:  # noqa: BLE001
        scene_label = scene if scene is not None else "?"
    try:
        pixel_sizes = tuple(img.physical_pixel_sizes)
    except Exception:  # noqa: BLE001
        pixel_sizes = (None, None, None)
    print(
        f"[io] {path.name}: scene={scene_label} TCZYX {full_shape} -> model input {kept_axes} {squeezed.shape} ({axis_map}), "
        f"channels={names}, pixel size ZYX={pixel_sizes}"
    )
    return squeezed


def resolve_inputs(path: Path) -> list[Path]:
    """Expand a glob pattern or directory into the list of input files."""
    if globlib.has_magic(str(path)):
        matches = sorted(Path(p) for p in globlib.glob(str(path)))
        if not matches:
            raise FileNotFoundError(f"no files match pattern {path}")
        return matches
    path = Path(path)
    if not path.exists():
        raise FileNotFoundError(path)
    if path.is_dir() and path.suffix.lower() != ".zarr":
        files = sorted(
            p for p in path.iterdir() if p.suffix.lower() in STACKABLE_SUFFIXES
        )
        if not files:
            raise FileNotFoundError(f"no readable image files in directory {path}")
        return files
    return [path]


def render_output(template: Path, source: Path, multi: bool) -> Path:
    """Per-input output path: {stem}/{dir} placeholders, else '<input-stem>_<name>' when multiple inputs."""
    text = str(template)
    if "{stem}" in text or "{dir}" in text:
        return Path(text.format(stem=source.stem, dir=source.parent))
    if not multi:
        return template
    return template.parent / f"{source.stem}_{template.name}"


def segment_batch(input_path, scene, channel, output_template, predict) -> dict:
    """Run predict(image, source_path) per input file, one labels output per input."""
    sources = resolve_inputs(input_path)
    multi = len(sources) > 1
    outputs, total = [], 0
    for i, src in enumerate(sources, 1):
        masks = predict(read_image(src, scene, channel), src)
        out = write_labels(render_output(Path(output_template), src, multi), masks)
        count = int(len(np.unique(masks)) - 1)
        total += count
        outputs.append(out)
        if multi:
            print(f"[{i}/{len(sources)}] {src.name} -> {out.name}: {count} objects")
    result = {}
    if multi:
        result["inputs"] = len(sources)
        result["masks"] = f"{len(outputs)} files in {outputs[0].parent.resolve()}"
    else:
        result["masks"] = str(outputs[0])
    result["objects"] = total
    return result


def write_labels(path: Path, labels: np.ndarray) -> Path:
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    dtype = np.uint16 if labels.max() < 2**16 else np.uint32
    tifffile.imwrite(path, labels.astype(dtype), compression="zlib")
    return path


def write_image(path: Path, image: np.ndarray) -> Path:
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    tifffile.imwrite(path, image, compression="zlib")
    return path
