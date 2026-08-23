from cli_core import io as core_io

from segmenter.tools.microsam.config import MicrosamConfig


def run_batch(cfg: MicrosamConfig, predict) -> dict:
    if cfg.io.input.image is None:
        raise ValueError("io.input.image is required")
    return core_io.segment_batch(
        cfg.io.input.image,
        cfg.io.input.scene,
        cfg.io.input.channel,
        cfg.io.output.masks,
        predict,
    )
