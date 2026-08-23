from pathlib import Path
from typing import Any, Literal

from cli_core.config import IOSection, StrictModel, ToolConfig
from pydantic import Field


class CellposeInput(StrictModel):
    image: Path | None = Field(
        None,
        description="required: .tif/.tiff/.npy file or directory of tiffs (stacked along first axis)",
    )


class CellposeOutput(StrictModel):
    masks: Path = Field(Path("masks.tif"), description="label image, 0 = background")


class CellposeIO(IOSection):
    input: CellposeInput = CellposeInput()
    output: CellposeOutput = CellposeOutput()


class CellposeModelOptions(StrictModel):
    pretrained_model: str = Field(
        "cpsam_v2",
        description="builtin model (cpsam_v2, cpdino, cpdino-vitb, cpsam) or path to custom weights",
    )
    device: str | None = Field(
        None,
        description="explicit torch device (e.g. cuda:1), overrides the --gpu flag",
    )
    use_bfloat16: bool = Field(True, description="run model weights in bfloat16")


class CellposeEvalOptions(StrictModel):
    batch_size: int = Field(8, description="number of tiles run per forward pass")
    resample: bool = Field(
        True,
        description="run dynamics at original image size (slower, smoother boundaries)",
    )
    channel_axis: int | None = Field(
        None, description="axis of image that is the channel axis (None = auto)"
    )
    z_axis: int | None = Field(
        None, description="axis of image that is the Z axis (None = auto)"
    )
    normalize: bool | dict[str, Any] = Field(
        True,
        description="percentile-normalize each channel to [1,99]; or dict, see cellpose normalize_default",
    )
    rescale: float | None = Field(
        None, description="resize factor, only used when diameter is None"
    )
    diameter: float | None = Field(
        None, description="cell diameter in px; image rescaled so cells are ~30 px"
    )
    flow_threshold: float = Field(
        0.4, description="max allowed flow error per mask (2D only)"
    )
    cellprob_threshold: float = Field(
        0.0, description="pixel probability cutoff; lower = more/larger masks"
    )
    do_3D: bool = Field(False, description="true 3D segmentation of 3D/4D input")
    anisotropy: float | None = Field(
        None,
        description="Z rescaling factor for 3D (e.g. 2.0 if Z sampled at half XY density)",
    )
    flow3D_smooth: float = Field(
        0, description="gaussian stddev for smoothing 3D flows"
    )
    stitch_threshold: float = Field(
        0.0,
        description=">0 with do_3D=false: segment planes in 2D, stitch masks across Z by IoU",
    )
    min_size: int = Field(15, description="drop masks smaller than this pixel count")
    max_size_fraction: float = Field(
        0.4, description="drop masks larger than this fraction of the image"
    )
    niter: int | None = Field(
        None, description="dynamics iterations (None = proportional to diameter)"
    )
    augment: bool = Field(False, description="average over flipped tile predictions")
    tile_overlap: float = Field(0.1, description="fraction of overlap between tiles")
    bsize: int | None = Field(
        None, description="tile size (None = model default: 256 for cpsam)"
    )


class CellposeOptions(StrictModel):
    model: CellposeModelOptions = CellposeModelOptions()
    eval: CellposeEvalOptions = CellposeEvalOptions()


class CellposeConfig(ToolConfig):
    tool: Literal["segmenter"] = "segmenter"
    algorithm: Literal["cellpose"] = "cellpose"
    io: CellposeIO = CellposeIO()
    options: CellposeOptions = CellposeOptions()
