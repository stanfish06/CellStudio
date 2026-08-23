from pathlib import Path

import numpy as np
import tifffile

TIFF_SUFFIXES = {".tif", ".tiff"}
# formats handled by the installed bioio reader plugins
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
}
STACKABLE_SUFFIXES = TIFF_SUFFIXES | BIOIO_SUFFIXES | {".npy"}


def read_image(
    path: Path, scene: int | str | None = None, channel: int | str | None = None
) -> np.ndarray:
    """Read an image/stack from most microscopy formats.

    tiff/npy are read raw; everything else goes through bioio (TCZYX, then
    singleton axes squeezed). A directory is read as sorted files stacked on
    axis 0. scene/channel select from multi-scene/multi-channel files and
    force the bioio path even for tiffs.
    """
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
        f"[io] {path.name}: scene={scene_label} dims=TCZYX {full_shape} -> {squeezed.shape}, "
        f"channels={names}, pixel size ZYX={pixel_sizes}"
    )
    return squeezed


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
