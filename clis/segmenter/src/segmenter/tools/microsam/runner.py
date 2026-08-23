from cli_core.registry import RunContext

from segmenter.tools.microsam.config import MicrosamConfig
from segmenter.tools.microsam.io import run_batch


def run(cfg: MicrosamConfig, ctx: RunContext) -> dict:
    from micro_sam.automatic_segmentation import (
        automatic_instance_segmentation,
        get_predictor_and_segmenter,
    )
    from micro_sam.instance_segmentation import InstanceSegmentationWithDecoder

    opts = cfg.options
    # explicit config device wins; else --gpu = auto-pick (cuda/mps), default = cpu
    device = opts.model.device or (None if ctx.gpu else "cpu")
    predictor, segmenter = get_predictor_and_segmenter(
        model_type=opts.model.model_type,
        checkpoint=opts.model.checkpoint,
        device=device,
        segmentation_mode=opts.model.segmentation_mode,
        is_tiled=opts.tiling.enabled,
    )
    # apg segmenters take no extra generate kwargs; ais/amg get their config section
    if "PromptGenerator" in type(segmenter).__name__:
        generate_kwargs = {}
    elif isinstance(segmenter, InstanceSegmentationWithDecoder):
        generate_kwargs = opts.ais.model_dump()
    else:
        generate_kwargs = opts.amg.model_dump()

    def predict(image, source):
        embeddings = cfg.io.output.embeddings
        if embeddings is not None:
            embeddings = embeddings / source.stem  # cache per input file
        return automatic_instance_segmentation(
            predictor,
            segmenter,
            input_path=image,
            embedding_path=embeddings,
            ndim=opts.run.ndim,
            tile_shape=tuple(opts.tiling.tile_shape) if opts.tiling.enabled else None,
            halo=tuple(opts.tiling.halo) if opts.tiling.enabled else None,
            batch_size=opts.run.batch_size,
            verbose=opts.run.verbose,
            **generate_kwargs,
        )

    return run_batch(cfg, predict)
