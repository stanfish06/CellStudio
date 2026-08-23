from cli_core.registry import RunContext

from segmenter.tools.cellpose.config import CellposeConfig
from segmenter.tools.cellpose.io import load_input, save_output


def run(cfg: CellposeConfig, ctx: RunContext) -> dict:
    from cellpose import models

    image = load_input(cfg)
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
    masks, _flows, _styles = model.eval(image, **cfg.options.eval.model_dump())
    return save_output(cfg, masks)
