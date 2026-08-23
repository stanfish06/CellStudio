from cli_core.app import build_app

from tracker.registry import REGISTRY

app = build_app(
    REGISTRY, "Cell tracking: label masks in, tracks (and tracked masks) out."
)


def main() -> None:
    app()
