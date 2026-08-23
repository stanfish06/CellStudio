from pathlib import Path
from typing import Literal

from cli_core.config import IOSection, StrictModel, ToolConfig
from pydantic import Field

Coefs = tuple[float, float, float, float, float]


class LaptrackInput(StrictModel):
    labels: Path | None = Field(
        None,
        description="required: label mask stack, TYX or TZYX .tif/.npy or directory of per-frame tiffs",
    )


class LaptrackOutput(StrictModel):
    tracks: Path = Field(
        Path("tracks.csv"),
        description="per-object rows: frame, label, track_id, tree_id",
    )
    splits: Path | None = Field(
        Path("splits.csv"),
        description="parent_track_id, child_track_id rows (null = skip)",
    )
    merges: Path | None = Field(
        Path("merges.csv"),
        description="parent_track_id, child_track_id rows (null = skip)",
    )
    tracked_labels: Path | None = Field(
        Path("tracked_labels.tif"),
        description="input masks relabeled with track_id + 1 (null = skip)",
    )


class LaptrackIO(IOSection):
    input: LaptrackInput = LaptrackInput()
    output: LaptrackOutput = LaptrackOutput()


class LaptrackTrackingOptions(StrictModel):
    """LapTrack parameters (Jaqaman 2008 LAP tracking)."""

    metric: str = Field(
        "sqeuclidean",
        description="linking cost metric, any scipy cdist metric; overlap mode uses metric_coefs instead",
    )
    cutoff: float = Field(
        225,
        description="linking cost cutoff; sqeuclidean = squared max distance. overlap-mode distances are ~[0,1], use e.g. 0.9",
    )
    gap_closing_metric: str = Field("sqeuclidean", description="metric for gap closing")
    gap_closing_cutoff: float | Literal[False] = Field(
        225, description="cost cutoff for gap closing, false = disable"
    )
    gap_closing_max_frame_count: int = Field(
        2, description="max skipped frames for gap closing"
    )
    splitting_metric: str = Field(
        "sqeuclidean", description="metric for splitting (division) candidates"
    )
    splitting_cutoff: float | Literal[False] = Field(
        False, description="cost cutoff for splitting, false = no splitting"
    )
    merging_metric: str = Field(
        "sqeuclidean", description="metric for merging candidates"
    )
    merging_cutoff: float | Literal[False] = Field(
        False, description="cost cutoff for merging, false = no merging"
    )
    track_start_cost: float | None = Field(
        None, description="cost of starting a track (null = auto-estimate)"
    )
    track_end_cost: float | None = Field(
        None, description="cost of ending a track (null = auto)"
    )
    segment_start_cost: float | None = Field(None, description="null = auto")
    segment_end_cost: float | None = Field(None, description="null = auto")
    no_splitting_cost: float | None = Field(
        None, description="cost of rejecting a split (null = auto)"
    )
    no_merging_cost: float | None = Field(
        None, description="cost of rejecting a merge (null = auto)"
    )
    alternative_cost_factor: float = Field(
        1.05, description="factor for alternative-cost estimation"
    )
    alternative_cost_percentile: float = Field(
        90, description="percentile for alternative-cost estimation"
    )
    alternative_cost_percentile_interpolation: str = Field(
        "lower", description="see numpy.percentile interpolation"
    )
    parallel_backend: Literal["serial", "ray"] = Field(
        "serial", description="backend for cost computation"
    )


class LaptrackOverlapOptions(StrictModel):
    """Coefficients (offset, overlap, iou, ratio_1, ratio_2): distance = offset + sum(coef * feature). Default = 1 - ratio_2."""

    metric_coefs: Coefs = Field(
        (1.0, 0.0, 0.0, 0.0, -1.0), description="linking distance coefficients"
    )
    gap_closing_metric_coefs: Coefs = Field(
        (1.0, 0.0, 0.0, 0.0, -1.0), description="gap-closing distance coefficients"
    )
    splitting_metric_coefs: Coefs = Field(
        (1.0, 0.0, 0.0, 0.0, -1.0), description="splitting distance coefficients"
    )
    merging_metric_coefs: Coefs = Field(
        (1.0, 0.0, 0.0, 0.0, -1.0), description="merging distance coefficients"
    )


class LaptrackOptions(StrictModel):
    mode: Literal["overlap", "centroid"] = Field(
        "overlap",
        description="overlap: link by mask overlap (OverLapTrack); centroid: link by centroid distance",
    )
    tracking: LaptrackTrackingOptions = LaptrackTrackingOptions()
    overlap: LaptrackOverlapOptions = LaptrackOverlapOptions()


class LaptrackConfig(ToolConfig):
    tool: Literal["tracker"] = "tracker"
    algorithm: Literal["laptrack"] = "laptrack"
    io: LaptrackIO = LaptrackIO()
    options: LaptrackOptions = LaptrackOptions()
