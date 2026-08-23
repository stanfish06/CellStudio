from cli_core.registry import Algorithm, Registry

from tracker.tools.laptrack.config import LaptrackConfig
from tracker.tools.ultrack.config import UltrackConfig

REGISTRY = Registry(
    "tracker",
    [
        Algorithm(
            name="laptrack",
            package="laptrack",
            summary="LAP tracking of label masks (overlap or centroid linking)",
            config_cls=LaptrackConfig,
            runner="tracker.tools.laptrack.runner:run",
        ),
        Algorithm(
            name="ultrack",
            package="ultrack",
            summary="ILP tracking over segmentation hypotheses derived from labels",
            config_cls=UltrackConfig,
            runner="tracker.tools.ultrack.runner:run",
        ),
    ],
)
