import numpy as np
from cli_core.registry import RunContext

from segmenter.tools.stardist.config import StardistConfig
from segmenter.tools.stardist.io import load_input, save_output


def run(cfg: StardistConfig, ctx: RunContext) -> dict:
    if not ctx.gpu:
        import tensorflow as tf

        tf.config.set_visible_devices([], "GPU")
    from csbdeep.utils import normalize
    from stardist.models import StarDist2D, StarDist3D

    opts = cfg.options
    model_cls = StarDist3D if opts.model.pretrained.startswith("3D") else StarDist2D
    model = model_cls.from_pretrained(opts.model.pretrained)
    image = load_input(cfg).astype(np.float32)

    predict_kwargs = opts.predict.model_dump()
    if predict_kwargs["n_tiles"] is not None:
        predict_kwargs["n_tiles"] = tuple(predict_kwargs["n_tiles"])

    def prepare(img: np.ndarray) -> np.ndarray:
        if not opts.normalize.enabled:
            return img
        axis = tuple(opts.normalize.axis) if opts.normalize.axis else None
        return normalize(img, opts.normalize.pmin, opts.normalize.pmax, axis=axis)

    def predict(img: np.ndarray) -> np.ndarray:
        if opts.big.enabled:
            if predict_kwargs["axes"] is None:
                raise ValueError(
                    "options.predict.axes is required when options.big.enabled is true"
                )
            labels, _details = model.predict_instances_big(
                img,
                block_size=opts.big.block_size,
                min_overlap=opts.big.min_overlap,
                context=opts.big.context,
                **predict_kwargs,
            )
        else:
            labels, _details = model.predict_instances(img, **predict_kwargs)
        return labels

    if opts.per_frame:
        masks = np.stack([predict(prepare(frame)) for frame in image])
    else:
        masks = predict(prepare(image))
    return save_output(cfg, masks)
