"""Общая обвязка тестов кодогенератора.

`generate.py` лежит не пакетом, а скриптом рядом — его и импортируем,
добавив buildgen/ в путь. Здесь же собраны пути проекта и фикстуры, чтобы
сами тесты говорили только о правилах, которые проверяют.
"""
import os
import sys

import pytest

BUILDGEN_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PROJECT_ROOT = os.path.normpath(os.path.join(BUILDGEN_DIR, ".."))
PROTO_DIR    = os.path.join(PROJECT_ROOT, "veldcore", "interface")
MODULES_ROOT = os.path.join(PROJECT_ROOT, "veldmodules")

sys.path.insert(0, BUILDGEN_DIR)


def make_universe(**packages) -> dict:
    """Вселенная типов из пакетов вида `alias=(("Msg", ...), origin)`.

    Собирается вручную, а не из .proto на диске: правила валидатора не зависят
    от того, какие сообщения существуют в проекте на самом деле, и тесты на них
    не должны краснеть от правки чужого .proto.
    """
    return {alias: {"messages": set(messages), "origin": origin, "source": f"<{alias}>"}
            for alias, (messages, origin) in packages.items()}


@pytest.fixture(scope="session")
def gen():
    """Сам кодогенератор."""
    import generate
    return generate


@pytest.fixture(scope="session")
def universe():
    """Минимальная вселенная типов для проверок в памяти."""
    return make_universe(core=(("ResourceOpened", "ResourceHandle"), "core"),
                         fs=(("FsReadRequest", "FsReadResult"), "core"))


@pytest.fixture
def core_errors(gen, universe):
    """Ошибки валидации в диалекте платформенного сервиса.

    Диска не касается: без `schema_dir` перекрёстной проверки зависимостей нет,
    поэтому правила проверяются целиком в памяти.
    """
    return lambda schema: gen.validate_core_schema(schema, universe)


@pytest.fixture(scope="session")
def real_universe(gen):
    """Вселенная типов самого проекта — для регрессий на живых схемах."""
    return gen.build_type_universe(PROTO_DIR, MODULES_ROOT)


@pytest.fixture(scope="session")
def real_schemas():
    """Все схемы проекта: (имя, путь, разобранный yaml, вид).

    Вид — `module` у wasm-модуля, `platform` у сервиса платформы: у них разные
    диалекты валидатора, и тесты обязаны звать правильный.
    """
    import yaml

    import generate

    found = []
    for name in sorted(os.listdir(MODULES_ROOT)):
        path = os.path.join(MODULES_ROOT, name, "schema.yaml")
        if os.path.exists(path):
            with open(path) as f:
                found.append((name, path, yaml.safe_load(f), "module"))
    for path in generate.iter_core_files(PROTO_DIR, ".schema.yaml"):
        with open(path) as f:
            schema = yaml.safe_load(f)
        found.append((schema.get("name"), path, schema, "platform"))
    return found
