from cli_core.registry import RunContext

from tracker.tools.laptrack.config import LaptrackConfig
from tracker.tools.laptrack.io import load_input, save_output


def run(cfg: LaptrackConfig, ctx: RunContext) -> dict:
    from laptrack import LapTrack, OverLapTrack

    if ctx.gpu:
        print("laptrack has no GPU path; running on CPU")

    labels = load_input(cfg)
    tracking = cfg.options.tracking.model_dump()

    if cfg.options.mode == "overlap":
        tracker = OverLapTrack(**tracking, **cfg.options.overlap.model_dump())
        track_df, split_df, merge_df = tracker.predict_overlap_dataframe(labels)
        track_df = track_df.reset_index()  # -> frame, label, tree_id, track_id columns
    else:
        import pandas as pd
        from skimage.measure import regionprops_table

        frames = []
        for t, frame_labels in enumerate(labels):
            props = pd.DataFrame(
                regionprops_table(frame_labels, properties=("label", "centroid"))
            )
            props["frame"] = t
            frames.append(props)
        points = pd.concat(frames, ignore_index=True)
        coordinate_cols = [c for c in points.columns if c.startswith("centroid-")]
        tracker = LapTrack(**tracking)
        track_df, split_df, merge_df = tracker.predict_dataframe(
            points, coordinate_cols, frame_col="frame", only_coordinate_cols=False
        )
        track_df = track_df.reset_index(drop=True)

    return save_output(cfg, labels, track_df, split_df, merge_df)
