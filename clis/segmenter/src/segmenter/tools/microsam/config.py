from pathlib import Path
from typing import Literal

from cli_core.config import IOSection, StrictModel, ToolConfig
from pydantic import Field

ModelType = Literal[
    "vit_t",
    "vit_b",
    "vit_l",
    "vit_h",
    "vit_t_lm",
    "vit_b_lm",
    "vit_l_lm",
    "vit_t_em_organelles",
    "vit_b_em_organelles",
    "vit_l_em_organelles",
    "vit_b_histopathology",
    "vit_l_histopathology",
    "vit_h_histopathology",
    "vit_b_medical_imaging",
]


class MicrosamInput(StrictModel):
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


class MicrosamOutput(StrictModel):
    masks: Path = Field(
        Path("masks.tif"),
        description="label image, 0 = background; with multiple inputs each file gets <input-stem>_<name>, or use {stem}/{dir} placeholders",
    )
    embeddings: Path | None = Field(
        None, description="cache dir for image embeddings (None = recompute each run)"
    )


class MicrosamIO(IOSection):
    input: MicrosamInput = MicrosamInput()
    output: MicrosamOutput = MicrosamOutput()


class MicrosamModelOptions(StrictModel):
    model_type: ModelType = Field(
        "vit_b_lm",
        description="_lm/_em/_histopathology models ship a decoder and support ais",
    )
    checkpoint: Path | None = Field(
        None, description="custom checkpoint path (None = download pretrained)"
    )
    device: str | None = Field(
        None,
        description="cuda / mps / cpu; overrides the --gpu flag (None = cpu, or auto with --gpu)",
    )
    segmentation_mode: Literal["amg", "ais", "apg"] | None = Field(
        None, description="None = auto: ais when the model has a decoder, else amg"
    )


class MicrosamTilingOptions(StrictModel):
    enabled: bool = Field(
        False, description="tiled embedding computation for large images"
    )
    tile_shape: list[int] = Field([1024, 1024], description="tile shape in pixels")
    halo: list[int] = Field([256, 256], description="overlap added around each tile")


class MicrosamRunOptions(StrictModel):
    ndim: int | None = Field(
        None,
        description="2 for a single 2D/RGB image, 3 for volumes/stacks (per-slice); None = auto",
    )
    batch_size: int = Field(
        1, description="batch size for embedding computation over tiles/planes"
    )
    verbose: bool = Field(True, description="print progress")


class MicrosamAmgOptions(StrictModel):
    """Applied when the resolved mode is amg."""

    pred_iou_thresh: float = Field(
        0.88, description="filter masks by predicted quality"
    )
    stability_score_thresh: float = Field(
        0.95, description="filter masks by stability under threshold changes"
    )
    box_nms_thresh: float = Field(0.7, description="NMS IoU cutoff for duplicate masks")
    crop_nms_thresh: float = Field(0.7, description="NMS IoU cutoff between crops")
    min_mask_region_area: int = Field(
        0, description="drop mask regions smaller than this"
    )
    with_background: bool = Field(
        True, description="treat the largest object as background and drop it"
    )


class MicrosamAisOptions(StrictModel):
    """Applied when the resolved mode is ais (decoder-based instance segmentation)."""

    center_distance_threshold: float = Field(
        0.5, description="cutoff on center-distance predictions for seeds"
    )
    boundary_distance_threshold: float = Field(
        0.5, description="cutoff on boundary-distance predictions for seeds"
    )
    foreground_threshold: float = Field(0.5, description="foreground mask cutoff")
    foreground_smoothing: float = Field(
        1.0, description="gaussian sigma for smoothing the foreground prediction"
    )
    distance_smoothing: float = Field(
        1.6, description="gaussian sigma for smoothing distance predictions"
    )
    min_size: int = Field(0, description="drop objects smaller than this pixel count")


class MicrosamOptions(StrictModel):
    model: MicrosamModelOptions = MicrosamModelOptions()
    tiling: MicrosamTilingOptions = MicrosamTilingOptions()
    run: MicrosamRunOptions = MicrosamRunOptions()
    amg: MicrosamAmgOptions = MicrosamAmgOptions()
    ais: MicrosamAisOptions = MicrosamAisOptions()


class MicrosamConfig(ToolConfig):
    tool: Literal["segmenter"] = "segmenter"
    algorithm: Literal["micro-sam"] = "micro-sam"
    io: MicrosamIO = MicrosamIO()
    options: MicrosamOptions = MicrosamOptions()
