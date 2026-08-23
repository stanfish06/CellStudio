from pathlib import Path
from typing import Literal

from cli_core.config import IOSection, StrictModel, ToolConfig
from pydantic import Field


class UltrackInput(StrictModel):
    labels: Path | None = Field(
        None,
        description="required: label mask stack, TYX or TZYX .tif/.npy or directory of per-frame tiffs",
    )


class UltrackOutput(StrictModel):
    tracks: Path = Field(
        Path("tracks.csv"),
        description="napari-style rows: track_id, t, (z,) y, x, id, parent_track_id, parent_id",
    )
    tracked_labels: Path | None = Field(
        Path("tracked_labels.tif"),
        description="segments relabeled by track_id (null = skip)",
    )


class UltrackIO(IOSection):
    input: UltrackInput = UltrackInput()
    output: UltrackOutput = UltrackOutput()


class UltrackDataOptions(StrictModel):
    """Intermediate data storage (ultrack works through a database)."""

    working_dir: Path = Field(
        Path(".ultrack"),
        description="directory for the sqlite database and metadata.toml",
    )
    n_workers: int = Field(1, description="workers for parallel processing")
    database: Literal["sqlite", "postgresql", "memory"] = Field(
        "sqlite", description="database backend"
    )
    address: str | None = Field(
        None, description="postgresql address, required when database = postgresql"
    )


class UltrackSegmentationOptions(StrictModel):
    """Segmentation hypotheses creation."""

    min_area: int = Field(
        100, description="candidate segments smaller than this are merged/removed"
    )
    min_area_factor: float = Field(
        4.0,
        description="foreground objects below min_area/min_area_factor removed entirely",
    )
    max_area: int = Field(
        1_000_000, description="candidate segments larger than this are merged/removed"
    )
    n_workers: int = Field(1, description="workers for segmentation hypotheses")
    min_frontier: float = Field(
        0.0,
        description="merge neighboring candidates when their average frontier is below this",
    )
    threshold: float = Field(0.5, description="foreground-map binarization threshold")
    max_noise: float = Field(
        0.0, description="upper limit of uniform noise added to the contour map"
    )
    random_seed: Literal["frame", "none"] = Field(
        "frame", description="seed for the noise"
    )
    ws_hierarchy: Literal["area", "dynamics", "volume"] = Field(
        "area", description="watershed hierarchy function"
    )
    anisotropy_penalization: float = Field(
        0.0, description="z-axis penalization; positive favors xy-plane segments"
    )


class UltrackLinkingOptions(StrictModel):
    """Candidate hypotheses linking."""

    max_distance: float = Field(
        15.0, description="max distance between segments in adjacent frames (key knob)"
    )
    n_workers: int = Field(1, description="workers for linking")
    max_neighbors: int = Field(5, description="max linking candidates per segment")
    distance_weight: float = Field(
        0.0, description="penalization: w_pq - weight * ||c_p - c_q||"
    )
    z_score_threshold: float = Field(
        5.0, description="z-score cutoff on intensity within neighboring masks"
    )


class UltrackTrackingOptions(StrictModel):
    """ILP selection of segments and links."""

    solver_name: Literal["GUROBI", "CBC", ""] = Field(
        "", description="empty = GUROBI if available, else CBC"
    )
    appear_weight: float = Field(
        -0.001, description="penalization for track appearance (negative)"
    )
    disappear_weight: float = Field(
        -0.001, description="penalization for track disappearance (negative)"
    )
    division_weight: float = Field(
        -0.001, description="penalization for division (negative)"
    )
    image_border_size: list[int] | None = Field(
        None, description="(z,)y,x border in px where appear/disappear is not penalized"
    )
    n_threads: int = Field(-1, description="solver threads (-1 = all)")
    window_size: int | None = Field(
        None, description="solve in time windows of this size (null = whole timelapse)"
    )
    overlap_size: int = Field(
        1, description="frames shared between consecutive windows"
    )
    solution_gap: float = Field(0.001, description="solver MIP gap")
    time_limit: int = Field(36000, description="solver time limit in seconds")
    method: int = Field(0, description="LP method passed to the solver")
    link_function: Literal["identity", "power"] = Field(
        "power", description="link weight transform"
    )
    power: float = Field(4, description="exponent of the power transform")
    bias: float = Field(-0.0, description="edge-weight bias (should be negative)")


class UltrackOptions(StrictModel):
    sigma: float | list[float] | None = Field(
        None,
        description="gaussian sigma for smoothing the contours derived from labels",
    )
    scale: list[float] | None = Field(
        None, description="physical scale per spatial axis, used for distances"
    )
    overwrite: Literal["all", "links", "solutions", "none"] = Field(
        "all", description="which database stages to recompute on re-run"
    )
    data: UltrackDataOptions = UltrackDataOptions()
    segmentation: UltrackSegmentationOptions = UltrackSegmentationOptions()
    linking: UltrackLinkingOptions = UltrackLinkingOptions()
    tracking: UltrackTrackingOptions = UltrackTrackingOptions()


class UltrackConfig(ToolConfig):
    tool: Literal["tracker"] = "tracker"
    algorithm: Literal["ultrack"] = "ultrack"
    io: UltrackIO = UltrackIO()
    options: UltrackOptions = UltrackOptions()
