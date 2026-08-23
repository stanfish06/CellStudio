import numpy as np
from cli_core import io as core_io

from segmenter.tools.stardist.config import StardistConfig


def load_input(cfg: StardistConfig) -> np.ndarray:
    if cfg.io.input.image is None:
        raise ValueError("io.input.image is required")
    return core_io.read_image(cfg.io.input.image)


def save_output(cfg: StardistConfig, masks: np.ndarray) -> dict:
    path = core_io.write_labels(cfg.io.output.masks, masks)
    return {"masks": str(path), "objects": int(len(np.unique(masks)) - 1)}
