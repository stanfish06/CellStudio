import numpy as np
from cli_core.registry import RunContext

from segmenter.tools.stardist.config import StardistConfig
from segmenter.tools.stardist.io import run_batch


def run(cfg: StardistConfig, ctx: RunContext) -> dict:
    if not ctx.gpu:
        import tensorflow as tf

        tf.config.set_visible_devices([], "GPU")
    from csbdeep.utils import normalize
    from stardist.models import StarDist2D, StarDist3D

    opts = cfg.options
    model_cls = StarDist3D if opts.model.pretrained.startswith("3D") else StarDist2D
    model = model_cls.from_pretrained(opts.model.pretrained)

    predict_kwargs = opts.predict.model_dump()
    if predict_kwargs["n_tiles"] is not None:
        predict_kwargs["n_tiles"] = tuple(predict_kwargs["n_tiles"])

    def prepare(img: np.ndarray) -> np.ndarray:
        if not opts.normalize.enabled:
            return img
        axis = tuple(opts.normalize.axis) if opts.normalize.axis else None
        return normalize(img, opts.normalize.pmin, opts.normalize.pmax, axis=axis)

    def predict_one(img: np.ndarray) -> np.ndarray:
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

    def predict(image, _source):
        image = image.astype(np.float32)
        if opts.per_frame:
            return np.stack([predict_one(prepare(frame)) for frame in image])
        return predict_one(prepare(image))

    return run_batch(cfg, predict)
