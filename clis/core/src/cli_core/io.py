from pathlib import Path

import numpy as np
import tifffile

IMAGE_SUFFIXES = {".tif", ".tiff"}


def read_image(path: Path) -> np.ndarray:
    path = Path(path)
    if not path.exists():
        raise FileNotFoundError(path)
    if path.is_dir():
        files = sorted(p for p in path.iterdir() if p.suffix.lower() in IMAGE_SUFFIXES)
        if not files:
            raise FileNotFoundError(f"no .tif/.tiff files in directory {path}")
        return np.stack([tifffile.imread(f) for f in files])
    if path.suffix.lower() == ".npy":
        return np.load(path)
    if path.suffix.lower() in IMAGE_SUFFIXES:
        return tifffile.imread(path)
    raise ValueError(
        f"unsupported input format '{path.suffix}' (use .tif/.tiff/.npy or a directory of tiffs)"
    )


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
