import importlib
import importlib.metadata
from collections.abc import Callable
from dataclasses import dataclass

from cli_core.config import ToolConfig


@dataclass(frozen=True)
class RunContext:
    gpu: bool = False  # from exec --gpu; runners map it to their library's GPU path


@dataclass(frozen=True)
class Algorithm:
    name: str
    package: str  # distribution name, for version lookup
    summary: str
    config_cls: type[ToolConfig]
    runner: str  # "module.path:function", imported only at exec time

    def version(self) -> str:
        try:
            return importlib.metadata.version(self.package)
        except importlib.metadata.PackageNotFoundError:
            return "not installed"

    def resolve_runner(self) -> Callable[[ToolConfig, RunContext], dict]:
        module_path, _, func_name = self.runner.partition(":")
        module = importlib.import_module(module_path)
        return getattr(module, func_name)


class Registry:
    def __init__(self, tool: str, algorithms: list[Algorithm]):
        self.tool = tool
        self._algorithms = {a.name: a for a in algorithms}

    def names(self) -> list[str]:
        return list(self._algorithms)

    def all(self) -> list[Algorithm]:
        return list(self._algorithms.values())

    def get(self, name: str) -> Algorithm:
        if name not in self._algorithms:
            available = ", ".join(self._algorithms)
            raise KeyError(f"unknown algorithm '{name}' (available: {available})")
        return self._algorithms[name]
