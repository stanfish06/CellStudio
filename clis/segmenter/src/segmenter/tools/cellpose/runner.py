from cli_core.registry import RunContext

from segmenter.tools.cellpose.config import CellposeConfig
from segmenter.tools.cellpose.io import run_batch


def run(cfg: CellposeConfig, ctx: RunContext) -> dict:
    from cellpose import models

    opts = cfg.options.model
    device = None
    if opts.device is not None:
        import torch

        device = torch.device(opts.device)
    model = models.CellposeModel(
        gpu=ctx.gpu,
        pretrained_model=opts.pretrained_model,
        device=device,
        use_bfloat16=opts.use_bfloat16,
    )
    eval_kwargs = cfg.options.eval.model_dump()

    def predict(image, _source):
        masks, _flows, _styles = model.eval(image, **eval_kwargs)
        return masks

    return run_batch(cfg, predict)
