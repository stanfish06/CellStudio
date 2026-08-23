import numpy as np
import pandas as pd
from cli_core import io as core_io

from tracker.tools.laptrack.config import LaptrackConfig

EDGE_COLUMNS = ["parent_track_id", "child_track_id"]


def load_input(cfg: LaptrackConfig) -> np.ndarray:
    if cfg.io.input.labels is None:
        raise ValueError("io.input.labels is required")
    labels = core_io.read_image(cfg.io.input.labels)
    if labels.ndim < 3:
        raise ValueError(f"expected a TYX or TZYX stack, got shape {labels.shape}")
    return labels


def relabel_by_track(labels: np.ndarray, track_df: pd.DataFrame) -> np.ndarray:
    # mask value = track_id + 1 so track 0 stays distinct from background
    out = np.zeros_like(labels, dtype=np.uint32)
    for frame, group in track_df.groupby("frame"):
        lut = np.zeros(int(labels[frame].max()) + 1, dtype=np.uint32)
        lut[group["label"].to_numpy(dtype=int)] = (
            group["track_id"].to_numpy(dtype=int) + 1
        )
        out[frame] = lut[labels[frame]]
    return out


def _write_edges(df: pd.DataFrame, path) -> None:
    if len(df.columns) == 0:
        df = pd.DataFrame(columns=EDGE_COLUMNS)
    df.to_csv(path, index=False)


def save_output(
    cfg: LaptrackConfig,
    labels: np.ndarray,
    track_df: pd.DataFrame,
    split_df: pd.DataFrame,
    merge_df: pd.DataFrame,
) -> dict:
    out = cfg.io.output
    out.tracks.parent.mkdir(parents=True, exist_ok=True)
    first = [
        c for c in ("frame", "label", "track_id", "tree_id") if c in track_df.columns
    ]
    rest = [c for c in track_df.columns if c not in first]
    track_df[first + rest].to_csv(out.tracks, index=False)
    result = {
        "tracks": str(out.tracks),
        "n_tracks": int(track_df["track_id"].nunique()),
        "n_splits": len(split_df),
        "n_merges": len(merge_df),
    }
    if out.splits is not None:
        _write_edges(split_df, out.splits)
        result["splits"] = str(out.splits)
    if out.merges is not None:
        _write_edges(merge_df, out.merges)
        result["merges"] = str(out.merges)
    if out.tracked_labels is not None:
        core_io.write_labels(out.tracked_labels, relabel_by_track(labels, track_df))
        result["tracked_labels"] = str(out.tracked_labels)
    return result
