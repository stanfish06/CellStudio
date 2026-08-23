import numpy as np
from cli_core import io as core_io

from segmenter.tools.cellpose.config import CellposeConfig


def load_input(cfg: CellposeConfig) -> np.ndarray:
    if cfg.io.input.image is None:
        raise ValueError("io.input.image is required")
    return core_io.read_image(
        cfg.io.input.image, cfg.io.input.scene, cfg.io.input.channel
    )


def save_output(cfg: CellposeConfig, masks: np.ndarray) -> dict:
    path = core_io.write_labels(cfg.io.output.masks, masks)
    return {"masks": str(path), "objects": int(len(np.unique(masks)) - 1)}
