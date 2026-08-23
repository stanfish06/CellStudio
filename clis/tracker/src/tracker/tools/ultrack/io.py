import numpy as np
import pandas as pd
from cli_core import io as core_io

from tracker.tools.ultrack.config import UltrackConfig


def load_input(cfg: UltrackConfig) -> np.ndarray:
    if cfg.io.input.labels is None:
        raise ValueError("io.input.labels is required")
    labels = core_io.read_image(cfg.io.input.labels)
    if labels.ndim < 3:
        raise ValueError(f"expected a TYX or TZYX stack, got shape {labels.shape}")
    return labels


def save_tracks(cfg: UltrackConfig, tracks_df: pd.DataFrame) -> dict:
    out = cfg.io.output
    out.tracks.parent.mkdir(parents=True, exist_ok=True)
    tracks_df.to_csv(out.tracks, index=False)
    return {"tracks": str(out.tracks), "n_tracks": int(tracks_df["track_id"].nunique())}


def save_tracked_labels(cfg: UltrackConfig, segments) -> dict:
    path = core_io.write_labels(cfg.io.output.tracked_labels, np.asarray(segments))
    return {"tracked_labels": str(path)}
