import os
import re
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


def _nvidia_gpu_present() -> bool:
    # nvidia-smi (and /dev/nvidiactl) can exist on GPU-less login nodes; only a
    # successful query proves a usable device
    if not shutil.which("nvidia-smi"):
        return False
    try:
        return (
            subprocess.run(
                ["nvidia-smi"], capture_output=True, check=False, timeout=10
            ).returncode
            == 0
        )
    except (OSError, subprocess.TimeoutExpired):
        return False


_CUDA_VARIANTS = ("cu126", "cu130")  # what torch>=2.13 is published for
_DEFAULT_CUDA = (
    "cu126"  # covers sm_70 (V100) through sm_90 (H100); cu130 is Blackwell-era
)
_CUDA_RE = re.compile(r"^\s*cuda:\s*[\"']?(cu\d{3})", re.MULTILINE)


def _yaml_cuda(args: list[str]) -> str | None:
    for arg in args:
        p = Path(arg)
        if p.suffix.lower() in (".yaml", ".yml") and p.is_file():
            m = _CUDA_RE.search(p.read_text())
            if m:
                return m.group(1)
    return None


def _torch_group(tool_dir: Path, args: list[str]) -> str:
    """cpu or a cuda variant, stable across commands so the tool env is not
    re-synced back and forth.

    Priority: CELLSTUDIO_TORCH env (cpu/gpu/cuXXX) > `cuda:` in the exec config
    yaml > marker file > NVIDIA detection. A cuda decision is persisted in the
    marker so shared-filesystem nodes without a GPU (e.g. cluster login nodes)
    keep using it; delete the marker to go back to cpu.
    """
    marker = tool_dir / ".torch-gpu"
    env = os.environ.get("CELLSTUDIO_TORCH")
    choice = None
    if env == "cpu":
        choice = "cpu"
    elif env == "gpu":
        choice = _DEFAULT_CUDA
    elif env in _CUDA_VARIANTS:
        choice = env
    if choice is None:
        choice = _yaml_cuda(args)
    if choice is None and marker.exists():
        content = marker.read_text().strip()
        choice = content if content in _CUDA_VARIANTS else _DEFAULT_CUDA
    if choice is None:
        choice = _DEFAULT_CUDA if _nvidia_gpu_present() else "cpu"
    if choice != "cpu":
        marker.write_text(choice + "\n")
    return choice


# formats only Bio-Formats can read; a config mentioning one turns on the bioformats group
_BIOFORMATS_SUFFIXES = (".ims", ".vsi", ".oir", ".oib")


def _needs_bioformats(tool_dir: Path, args: list[str]) -> bool:
    marker = tool_dir / ".bioformats"
    if marker.exists():
        return True
    for arg in args:
        p = Path(arg)
        if p.suffix.lower() in (".yaml", ".yml") and p.is_file():
            text = p.read_text().lower()
            if any(suffix in text for suffix in _BIOFORMATS_SUFFIXES):
                marker.touch(exist_ok=True)
                return True
    return False


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
        groups = set(data.get("dependency-groups", {}))
        tools[path.name] = {
            "dir": path,
            "description": meta.get("description", ""),
            # cpu/gpu-cuXXX dependency-groups switch between cpu and cuda torch builds
            "gpu_groups": "cpu" in groups
            and any(g.startswith("gpu-cu") for g in groups),
            "groups": groups,
            "bioformats_group": "bioformats" in groups,
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
    if tools[tool]["bioformats_group"] and _needs_bioformats(
        tools[tool]["dir"], ctx.args
    ):
        cmd += ["--group", "bioformats"]
    if tools[tool]["gpu_groups"]:
        choice = _torch_group(tools[tool]["dir"], ctx.args)
        if choice != "cpu":
            if f"gpu-{choice}" not in tools[tool]["groups"]:
                available = ", ".join(
                    sorted(
                        g.removeprefix("gpu-")
                        for g in tools[tool]["groups"]
                        if g.startswith("gpu-cu")
                    )
                )
                typer.secho(
                    f"error: {tool} has no gpu-{choice} group (available cuda builds: {available})",
                    fg="red",
                    err=True,
                )
                raise typer.Exit(1)
            cmd += ["--no-group", "cpu", "--group", f"gpu-{choice}"]
        elif "--gpu" in ctx.args:
            typer.secho(
                "no NVIDIA GPU detected; keeping cpu torch", fg="yellow", err=True
            )
    cmd += [tool, *ctx.args]
    env = {k: v for k, v in os.environ.items() if k != "VIRTUAL_ENV"}
    # cluster cuda/cudnn modules on LD_LIBRARY_PATH outrank the pip wheels' RUNPATHs and
    # mix library versions inside the tool env; the pip stack is self-contained, drop them
    if tools[tool]["gpu_groups"] and env.get("LD_LIBRARY_PATH"):
        kept = [
            p
            for p in env["LD_LIBRARY_PATH"].split(":")
            if not re.search(r"cudnn|cuda", p, re.IGNORECASE)
        ]
        dropped = env["LD_LIBRARY_PATH"].count(":") + 1 - len(kept)
        if dropped:
            typer.secho(
                f"dropped {dropped} cuda/cudnn module path(s) from LD_LIBRARY_PATH for {tool}",
                fg="yellow",
                err=True,
            )
            env["LD_LIBRARY_PATH"] = ":".join(kept)
    raise typer.Exit(subprocess.run(cmd, env=env, check=False).returncode)


def main() -> None:
    app()


if __name__ == "__main__":
    sys.exit(main())
