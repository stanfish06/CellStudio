import types
import typing
from pathlib import Path
from typing import Literal

import yaml
from pydantic import BaseModel, ConfigDict, Field


class StrictModel(BaseModel):
    model_config = ConfigDict(extra="forbid")


class IOSection(StrictModel):
    """Subclasses define `input` and `output` models with path fields."""


class ToolConfig(StrictModel):
    tool: str
    algorithm: str
    cuda: Literal["cu126", "cu130"] | None = Field(
        None,
        description="cuda build for the tool env, used by cellstudio dispatch on GPU machines (null = auto, cu126)",
    )


def _literal_choices(annotation) -> list | None:
    # unwrap Optional/Union and report Literal choices if any branch is a Literal
    origin = typing.get_origin(annotation)
    if origin is typing.Literal:
        return list(typing.get_args(annotation))
    if origin in (typing.Union, types.UnionType):
        choices = []
        for arg in typing.get_args(annotation):
            sub = _literal_choices(arg)
            if sub:
                choices.extend(sub)
        return choices or None
    return None


def _format_scalar(value) -> str:
    if isinstance(value, Path):
        value = str(value)
    if isinstance(value, tuple):
        value = list(value)
    text = yaml.safe_dump(value, default_flow_style=True, width=10**6).strip()
    # pyyaml appends a "..." document-end marker after bare scalars
    return text.removesuffix("...").strip()


def _emit(model: BaseModel, lines: list[str], indent: int) -> None:
    pad = "  " * indent
    for name, field in type(model).model_fields.items():
        value = getattr(model, name)
        comment_parts = []
        if field.description:
            comment_parts.append(field.description)
        choices = _literal_choices(field.annotation)
        if choices and len(choices) > 1:
            comment_parts.append("choices: " + ", ".join(str(c) for c in choices))
        if comment_parts:
            lines.append(f"{pad}# {' | '.join(comment_parts)}")
        if isinstance(value, BaseModel):
            lines.append(f"{pad}{name}:")
            _emit(value, lines, indent + 1)
        else:
            lines.append(f"{pad}{name}: {_format_scalar(value)}")


def dump_commented_yaml(model: BaseModel, header: list[str] | None = None) -> str:
    lines = [f"# {line}" for line in header or []]
    _emit(model, lines, 0)
    return "\n".join(lines) + "\n"


def load_yaml(path: Path) -> dict:
    with open(path) as f:
        data = yaml.safe_load(f)
    if not isinstance(data, dict):
        raise ValueError(f"{path}: expected a YAML mapping at top level")
    return data
