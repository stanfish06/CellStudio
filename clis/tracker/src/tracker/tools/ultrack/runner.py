from cli_core.registry import RunContext

from tracker.tools.ultrack.config import UltrackConfig
from tracker.tools.ultrack.io import load_input, save_tracked_labels, save_tracks


def run(cfg: UltrackConfig, ctx: RunContext) -> dict:
    from ultrack import MainConfig, to_tracks_layer, track, tracks_to_zarr

    if ctx.gpu:
        print("ultrack's ILP solve is CPU-bound; running on CPU")

    opts = cfg.options
    # overwrite != "all" reuses earlier stages from the database; on a fresh
    # working dir those tables do not exist and ultrack fails mid-run
    if opts.overwrite != "all":
        db = opts.data.working_dir / "data.db"
        if opts.data.database == "memory":
            raise ValueError(
                f"options.overwrite = {opts.overwrite!r} needs a persisted database; "
                "database = memory starts empty every run, use overwrite = all"
            )
        if opts.data.database == "sqlite" and not db.exists():
            raise ValueError(
                f"options.overwrite = {opts.overwrite!r} reuses a previous run, "
                f"but {db} does not exist; use overwrite = all for a fresh run"
            )

    labels = load_input(cfg)
    opts.data.working_dir.mkdir(parents=True, exist_ok=True)

    tracking = opts.tracking.model_dump()
    if tracking["image_border_size"] is not None:
        tracking["image_border_size"] = tuple(tracking["image_border_size"])
    main_config = MainConfig.model_validate(
        {
            "data": opts.data.model_dump(),
            "segmentation": opts.segmentation.model_dump(),
            "linking": opts.linking.model_dump(),
            "tracking": tracking,
        }
    )

    track(
        main_config,
        labels=labels,
        sigma=opts.sigma,
        scale=opts.scale,
        overwrite=opts.overwrite,
    )

    tracks_df, _graph = to_tracks_layer(main_config)
    result = save_tracks(cfg, tracks_df)
    if cfg.io.output.tracked_labels is not None:
        result |= save_tracked_labels(cfg, tracks_to_zarr(main_config, tracks_df))
    return result
