from cli_core import io as core_io

from segmenter.tools.stardist.config import StardistConfig


def run_batch(cfg: StardistConfig, predict) -> dict:
    if cfg.io.input.image is None:
        raise ValueError("io.input.image is required")
    return core_io.segment_batch(
        cfg.io.input.image,
        cfg.io.input.scene,
        cfg.io.input.channel,
        cfg.io.output.masks,
        predict,
    )
