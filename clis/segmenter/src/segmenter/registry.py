from cli_core.registry import Algorithm, Registry

from segmenter.tools.cellpose.config import CellposeConfig
from segmenter.tools.microsam.config import MicrosamConfig
from segmenter.tools.stardist.config import StardistConfig

REGISTRY = Registry(
    "segmenter",
    [
        Algorithm(
            name="cellpose",
            package="cellpose",
            summary="Cellpose-SAM generalist segmentation (2D/3D)",
            config_cls=CellposeConfig,
            runner="segmenter.tools.cellpose.runner:run",
        ),
        Algorithm(
            name="stardist",
            package="stardist",
            summary="star-convex object detection, best for roundish nuclei",
            config_cls=StardistConfig,
            runner="segmenter.tools.stardist.runner:run",
        ),
        Algorithm(
            name="micro-sam",
            package="micro-sam",
            summary="Segment Anything fine-tuned for microscopy (amg/ais/apg)",
            config_cls=MicrosamConfig,
            runner="segmenter.tools.microsam.runner:run",
        ),
    ],
)
