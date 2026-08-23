from cli_core.app import build_app

from segmenter.registry import REGISTRY

app = build_app(
    REGISTRY, "Automatic cell segmentation: image stacks in, label masks out."
)


def main() -> None:
    app()
