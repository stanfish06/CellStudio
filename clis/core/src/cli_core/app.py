import time
from pathlib import Path
from typing import Annotated

import typer
from pydantic import ValidationError

from cli_core.config import dump_commented_yaml, load_yaml
from cli_core.registry import Registry, RunContext


def _fail(message: str) -> typer.Exit:
    typer.secho(f"error: {message}", fg="red", err=True)
    return typer.Exit(1)


def build_app(registry: Registry, description: str) -> typer.Typer:
    app = typer.Typer(help=description, add_completion=False, no_args_is_help=True)

    @app.command("list")
    def list_() -> None:
        """List available algorithms and their installed versions."""
        rows = [(a.name, a.version(), a.summary) for a in registry.all()]
        name_w = max(len(r[0]) for r in rows)
        ver_w = max(len(r[1]) for r in rows)
        for name, version, summary in rows:
            typer.echo(f"{name:<{name_w}}  {version:<{ver_w}}  {summary}")

    @app.command("init")
    def init(
        algo: Annotated[
            str, typer.Option("--algo", "-a", help="algorithm name (see `list`)")
        ],
        output: Annotated[
            Path | None,
            typer.Option("--output", "-o", help="config path (default: ./<algo>.yaml)"),
        ] = None,
        force: Annotated[
            bool, typer.Option("--force", "-f", help="overwrite existing file")
        ] = False,
    ) -> None:
        """Emit a config yaml with the full option mapping for an algorithm."""
        try:
            algorithm = registry.get(algo)
        except KeyError as e:
            raise _fail(str(e.args[0])) from None
        path = output or Path(f"{algo}.yaml")
        if path.exists() and not force:
            raise _fail(f"{path} exists (use --force to overwrite)")
        header = [
            f"{registry.tool} config > algorithm: {algorithm.name} ({algorithm.package} {algorithm.version()})",
            f"edit io paths and options, then run: {registry.tool} exec {path.name}",
        ]
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(dump_commented_yaml(algorithm.config_cls(), header))
        typer.echo(f"wrote {path}")

    @app.command("exec")
    def exec_(
        config: Annotated[
            Path,
            typer.Argument(exists=True, dir_okay=False, help="config yaml from `init`"),
        ],
        gpu: Annotated[
            bool,
            typer.Option(
                "--gpu",
                help="run on GPU where the algorithm supports it (default: CPU)",
            ),
        ] = False,
    ) -> None:
        """Run an algorithm as described by a config yaml."""
        data = load_yaml(config)
        algo_name = data.get("algorithm")
        if not algo_name:
            raise _fail(f"{config}: missing 'algorithm' key")
        try:
            algorithm = registry.get(algo_name)
        except KeyError as e:
            raise _fail(str(e.args[0])) from None
        try:
            cfg = algorithm.config_cls.model_validate(data)
        except ValidationError as e:
            raise _fail(f"invalid config {config}:\n{e}") from None
        run = algorithm.resolve_runner()
        device = "gpu" if gpu else "cpu"
        typer.echo(
            f"[{registry.tool}] running {algorithm.name} ({algorithm.package} {algorithm.version()}) on {device}"
        )
        start = time.perf_counter()
        result = run(cfg, RunContext(gpu=gpu))
        elapsed = time.perf_counter() - start
        for key, value in (result or {}).items():
            typer.echo(f"{key}: {value}")
        typer.echo(f"done in {elapsed:.1f}s")

    return app
