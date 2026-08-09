"""Отслеживание .proto сборочными скриптами.

Cargo следит за изменениями только внутри пакета, а .proto лежат вне
generated/-крейтов: без `cargo:rerun-if-changed` build.rs не перезапускается,
prost оставляет прежний вывод, и типы крейта молча расходятся с .proto. Ошибка
при этом всплывает не там, где сделана правка, — поэтому правило проверяется
на всех сборочных шаблонах сразу, а не на том, где его однажды забыли.
"""
import os
import re

from jinja2 import Environment, FileSystemLoader

from conftest import BUILDGEN_DIR

TEMPLATES_DIR = os.path.join(BUILDGEN_DIR, "templates")

# Шаблоны сборочных скриптов и данные, при которых они вообще компилируют
# .proto: у модуля без собственных типов компилировать нечего.
BUILD_TEMPLATES = {
    "build.rs.j2": {"local_proto_path": "../types.proto", "include_dirs": ["../../interface"]},
    "wrap_build.rs.j2": {"include_proto_dir": "../../../../../veldcore/interface"},
    "host_build.rs.j2": {"proto_dir_rel": "../interface", "proto_files": ["core.proto"]},
}


def render(name: str, data: dict) -> str:
    env = Environment(loader=FileSystemLoader(TEMPLATES_DIR))
    return env.get_template(name).render(**data)


def compiled_protos(source: str) -> set:
    """Пути .proto, которые скрипт отдаёт prost на компиляцию.

    Строки-директивы cargo сюда не считаются: в них те же пути, и без отсева
    проверка сравнивала бы множество с самим собой.
    """
    return set(re.findall(r'"(?!cargo:)([^"]+\.proto)"', source))


def watched_protos(source: str) -> set:
    """Пути .proto, за которыми скрипт просит cargo следить.

    Считаются и статичные строки, и вывод в цикле по списку файлов: во втором
    случае имени пути в шаблоне нет, но следят ровно за компилируемыми.
    """
    watched = set(re.findall(r'cargo:rerun-if-changed=([^"\\]+\.proto)', source))
    if re.search(r'cargo:rerun-if-changed=\{\}", *proto', source):
        watched |= compiled_protos(source)
    return watched


def test_every_compiled_proto_is_watched():
    for name, data in BUILD_TEMPLATES.items():
        source = render(name, data)
        compiled = compiled_protos(source)
        assert compiled, f"{name}: шаблон перестал компилировать .proto — проверять нечего"
        missing = compiled - watched_protos(source)
        assert not missing, f"{name}: не следит за {sorted(missing)}"


def test_all_build_templates_are_covered():
    """Новый сборочный шаблон должен попасть под проверку выше."""
    on_disk = {name for name in os.listdir(TEMPLATES_DIR) if name.endswith("build.rs.j2")}
    assert on_disk == set(BUILD_TEMPLATES)
