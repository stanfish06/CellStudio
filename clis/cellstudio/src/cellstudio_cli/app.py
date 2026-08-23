import os
import shutil
import subprocess
import sys
from pathlib import Path

import tomllib
import typer

app = typer.Typer(
    help="CellStudio CLI: dispatches tool CLIs (segmenter, tracker, ...) in their own environments.",
    add_completion=False,
    no_args_is_help=True,
)
tool_app = typer.Typer(help="Inspect available tools.", no_args_is_help=True)
app.add_typer(tool_app, name="tool")

_NON_TOOL_DIRS = {"core", "cellstudio"}


def _is_clis_dir(path: Path) -> bool:
    return (path / "core" / "pyproject.toml").is_file() and (
        path / "cellstudio"
    ).is_dir()


def find_clis_dir() -> Path:
    env = os.environ.get("CELLSTUDIO_CLIS_DIR")
    if env:
        return Path(env)
    # editable install: this file lives at clis/cellstudio/src/cellstudio_cli/app.py
    for parent in Path(__file__).resolve().parents:
        if _is_clis_dir(parent):
            return parent
    # tool install (non-editable): uv records the source dir in the tool venv's receipt
    receipt = Path(sys.prefix) / "uv-receipt.toml"
    if receipt.is_file():
        for req in (
            tomllib.loads(receipt.read_text()).get("tool", {}).get("requirements", [])
        ):
            src = req.get("directory") or req.get("editable") or req.get("path")
            if src and _is_clis_dir(Path(src).parent):
                return Path(src).parent
    # fallback: walk up from cwd looking for a clis/ directory
    for parent in [Path.cwd(), *Path.cwd().parents]:
        if _is_clis_dir(parent / "clis"):
            return parent / "clis"
        if _is_clis_dir(parent):
            return parent
    typer.secho(
        "error: cannot locate the clis/ directory — set CELLSTUDIO_CLIS_DIR=<repo>/clis, "
        "or reinstall from the repo: uv tool install --reinstall <repo>/clis/cellstudio",
        fg="red",
        err=True,
    )
    raise typer.Exit(1)


def discover_tools() -> dict[str, dict]:
    tools = {}
    for path in sorted(find_clis_dir().iterdir()):
        if not path.is_dir() or path.name in _NON_TOOL_DIRS:
            continue
        pyproject = path / "pyproject.toml"
        if not pyproject.is_file():
            continue
        data = tomllib.loads(pyproject.read_text())
        meta = data.get("project", {})
        scripts = meta.get("scripts", {})
        if path.name not in scripts:
            continue  # a tool must expose a console script named after its directory
        groups = data.get("dependency-groups", {})
        tools[path.name] = {
            "dir": path,
            "description": meta.get("description", ""),
            # cpu/gpu dependency-groups switch between cpu and cuda torch builds
            "gpu_groups": "cpu" in groups and "gpu" in groups,
        }
    return tools


@tool_app.command("list")
def tool_list() -> None:
    """List available tool CLIs."""
    tools = discover_tools()
    if not tools:
        typer.echo("no tools found")
        return
    width = max(len(name) for name in tools)
    for name, info in tools.items():
        typer.echo(f"{name:<{width}}  {info['description']}")


@app.command(
    "run",
    # no own --help: everything after the tool name passes through to the tool CLI
    context_settings={
        "allow_extra_args": True,
        "ignore_unknown_options": True,
        "help_option_names": [],
    },
)
def run(
    ctx: typer.Context,
    tool: str = typer.Argument(..., help="tool name (see `cellstudio tool list`)"),
) -> None:
    """Run a tool CLI inside its own environment: cellstudio run segmenter list|init|exec ..."""
    tools = discover_tools()
    if tool not in tools:
        available = ", ".join(tools) or "none"
        typer.secho(
            f"error: unknown tool '{tool}' (available: {available})", fg="red", err=True
        )
        raise typer.Exit(1)
    cmd = ["uv", "run", "--project", str(tools[tool]["dir"])]
    if "--gpu" in ctx.args and tools[tool]["gpu_groups"]:
        # switch the tool env to cuda torch, but only where a cuda device exists
        if shutil.which("nvidia-smi") or Path("/dev/nvidiactl").exists():
            cmd += ["--no-group", "cpu", "--group", "gpu"]
        else:
            typer.secho(
                "no NVIDIA GPU detected; keeping cpu torch", fg="yellow", err=True
            )
    cmd += [tool, *ctx.args]
    env = {k: v for k, v in os.environ.items() if k != "VIRTUAL_ENV"}
    raise typer.Exit(subprocess.run(cmd, env=env, check=False).returncode)


def main() -> None:
    app()


if __name__ == "__main__":
    sys.exit(main())
