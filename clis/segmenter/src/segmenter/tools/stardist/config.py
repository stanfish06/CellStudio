from pathlib import Path
from typing import Literal

from cli_core.config import IOSection, StrictModel, ToolConfig
from pydantic import Field

PretrainedModel = Literal[
    "2D_versatile_fluo", "2D_versatile_he", "2D_paper_dsb2018", "2D_demo", "3D_demo"
]


class StardistInput(StrictModel):
    image: Path | None = Field(
        None,
        description="required: microscopy image: tif/ome-tiff, nd2, czi, lif, dv, png/jpg, ome-zarr, npy, a directory, or a glob pattern like '/data/*.ims' (each file segmented separately; quote patterns starting with *)",
    )
    scene: int | str | None = Field(
        None,
        description="scene to read from multi-scene formats, index or name (null = first)",
    )
    channel: int | str | None = Field(
        None, description="channel to read, index or name (null = all channels)"
    )


class StardistOutput(StrictModel):
    masks: Path = Field(
        Path("masks.tif"),
        description="label image, 0 = background; with multiple inputs each file gets <input-stem>_<name>, or use {stem}/{dir} placeholders",
    )


class StardistIO(IOSection):
    input: StardistInput = StardistInput()
    output: StardistOutput = StardistOutput()


class StardistModelOptions(StrictModel):
    pretrained: PretrainedModel = Field(
        "2D_versatile_fluo",
        description="registered pretrained model; 3D_* names use StarDist3D",
    )


class StardistNormalizeOptions(StrictModel):
    enabled: bool = Field(
        True, description="percentile-normalize the image before prediction"
    )
    pmin: float = Field(1.0, description="lower percentile")
    pmax: float = Field(99.8, description="upper percentile")
    axis: list[int] | None = Field(
        None, description="axes to normalize jointly (None = all)"
    )


class StardistPredictOptions(StrictModel):
    axes: str | None = Field(
        None,
        description="axes string of the image, e.g. YX, ZYX, YXC (None = model default)",
    )
    prob_thresh: float | None = Field(
        None, description="object probability cutoff (None = model's optimized value)"
    )
    nms_thresh: float | None = Field(
        None, description="NMS overlap cutoff (None = model's optimized value)"
    )
    scale: float | list[float] | None = Field(
        None,
        description="rescale image internally, output mapped back; scalar or per-axis e.g. [4, 1, 1] for ZYX — the size/anisotropy knob (no diameter param in stardist)",
    )
    n_tiles: list[int] | None = Field(
        None, description="tile counts per axis to limit memory (None = no tiling)"
    )
    sparse: bool = Field(True, description="memory-efficient sparse aggregation")
    overlap_label: int | None = Field(
        None, description="label value for overlapping regions (None = off)"
    )


class StardistBigOptions(StrictModel):
    enabled: bool = Field(
        False,
        description="block-wise prediction for images that do not fit in memory (requires predict.axes)",
    )
    block_size: int = Field(
        4096, description="process blocks of this size; every object must fit a block"
    )
    min_overlap: int = Field(
        128,
        description="guaranteed block overlap; every object must be smaller than this",
    )
    context: int | None = Field(
        128, description="extra context discarded at block edges (None = auto)"
    )


class StardistOptions(StrictModel):
    model: StardistModelOptions = StardistModelOptions()
    per_frame: bool = Field(
        False,
        description="treat the first axis as frames and run the 2D model on each frame independently",
    )
    normalize: StardistNormalizeOptions = StardistNormalizeOptions()
    predict: StardistPredictOptions = StardistPredictOptions()
    big: StardistBigOptions = StardistBigOptions()


class StardistConfig(ToolConfig):
    tool: Literal["segmenter"] = "segmenter"
    algorithm: Literal["stardist"] = "stardist"
    io: StardistIO = StardistIO()
    options: StardistOptions = StardistOptions()
