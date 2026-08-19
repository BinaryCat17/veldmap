"""Зависимости wrap-крейта.

Файл, включённый и в модуль, и в его wrap (`#[path]` в wraps/rust/src/wrap.rs),
компилируется дважды — в двух разных крейтах. Крейты эти собираются по разным
манифестам, поэтому библиотека, которой такой файл пользуется, обязана стоять в
обоих: в модуле её объявляет `rust.dependencies`, в wrap — `rust.wrap_dependencies`.

Списки нарочно разные. Наследуй wrap весь список модуля — потребитель чужого
API тянул бы за собой и то, чем производитель пользуется только у себя; а
забудь его вовсе — общий файл перестал бы собираться, и увидеть это можно было
бы только по красной сборке потребителя, а не автора правки. Последнее и
проверяется здесь по живому дереву, а не на выдуманном модуле: правило это про
файлы, которых в проекте считанные единицы, и назвать их поимённо дешевле, чем
описать.
"""
import os
import re

import yaml
from jinja2 import Environment, FileSystemLoader

from conftest import BUILDGEN_DIR, MODULES_ROOT

TEMPLATES_DIR = os.path.join(BUILDGEN_DIR, "templates")

# Крейты, которые есть у всякого модуля и всякого wrap по построению
# (см. Cargo.toml.j2 и wrap_Cargo.toml.j2), — объявлять их в config.yaml не надо.
ALWAYS_THERE = {"veldsdk", "prost", "serde", "serde_json"}

# Не крейты, а пути внутри своего.
NOT_CRATES = {"crate", "self", "super", "std", "core", "alloc"}


def render_wrap_cargo(**data) -> str:
    env = Environment(loader=FileSystemLoader(TEMPLATES_DIR))
    return env.get_template("wrap_Cargo.toml.j2").render(**data)


def module_config(name: str) -> dict:
    path = os.path.join(MODULES_ROOT, name, "config.yaml")
    with open(path) as f:
        return (yaml.safe_load(f) or {}).get("rust", {}) or {}


def shared_files() -> list:
    """Файлы модуля, включённые в его wrap: (модуль, путь к файлу).

    Ищутся по `#[path]` в самом wrap.rs, а не по списку: список устарел бы
    раньше кода, а атрибут — это и есть объявление такого включения.
    """
    found = []
    for name in sorted(os.listdir(MODULES_ROOT)):
        wrap = os.path.join(MODULES_ROOT, name, "wraps", "rust", "src", "wrap.rs")
        if not os.path.exists(wrap):
            continue
        with open(wrap) as f:
            source = f.read()
        for rel in re.findall(r'#\[path\s*=\s*"([^"]+)"\]', source):
            path = os.path.normpath(os.path.join(os.path.dirname(wrap), rel))
            if os.path.exists(path):
                found.append((name, path))
    return found


def crates_used(path: str) -> set:
    """Внешние крейты, которыми пользуется файл, — по его `use`."""
    with open(path) as f:
        source = f.read()
    heads = set(re.findall(r'^\s*(?:pub\s+)?use\s+([A-Za-z_][A-Za-z0-9_]*)', source, re.M))
    return {head for head in heads if head not in NOT_CRATES and head not in ALWAYS_THERE}


def test_declared_wrap_dependency_reaches_the_manifest():
    source = render_wrap_cargo(
        api_crate_name="veldmap-globe-wrap",
        version="0.1.0",
        wrap_dependencies={"glam": '{ version = "0.30", features = ["scalar-math"] }'},
    )
    assert 'glam = { version = "0.30", features = ["scalar-math"] }' in source


def test_wrap_manifest_without_declarations_stays_as_before():
    """Пустой список ничего не добавляет: у большинства модулей общих файлов нет."""
    source = render_wrap_cargo(api_crate_name="x-wrap", version="0.1.0", wrap_dependencies={})
    assert "glam" not in source
    assert 'prost = "0.14"' in source


def test_both_lists_are_read_the_same_way(gen):
    """Одна форма записи в config.yaml значит в обоих списках одно и то же:
    разойдись перевод — один и тот же файл компилировался бы в двух крейтах с
    разными библиотеками."""
    declared = {"glam": {"version": "0.25", "features": ["scalar-math"]}, "log": "0.4"}
    assert gen.cargo_deps(declared) == {
        "glam": '{ version = "0.25", features = ["scalar-math"] }',
        "log": '"0.4"',
    }
    assert gen.cargo_deps(None) == {}


def test_every_shared_file_has_its_crates_in_both_lists():
    """Главная проверка: что общий файл `use`-ает, то объявлено и модулю, и wrap.

    Забыть здесь легко и незаметно — модуль соберётся, а wrap развалится уже у
    потребителя, в чужой сборке.
    """
    shared = shared_files()
    assert shared, "включений через #[path] не нашлось — проверять нечего, и это подозрительно"

    for name, path in shared:
        rust = module_config(name)
        module_deps = rust.get("dependencies", {}) or {}
        wrap_deps = rust.get("wrap_dependencies", {}) or {}
        for crate in sorted(crates_used(path)):
            where = f"{name}: общий файл {os.path.basename(path)} пользуется '{crate}'"
            assert crate in module_deps, f"{where}, но у модуля он не объявлен"
            assert crate in wrap_deps, f"{where}, но у wrap он не объявлен"
            assert module_deps[crate] == wrap_deps[crate], f"{where} разных версий в двух списках"


def test_shared_crates_agree_across_modules():
    """Крейт, попавший в общий файл, становится частью чужого API: типы из него
    видит и потребитель wrap-крейта. Версия у всех, кто его объявляет, обязана
    быть одна — иначе `DVec3` производителя и `DVec3` потребителя окажутся
    разными типами, и сборка развалится там, где их сводят."""
    shared_crates = set()
    for name, _ in shared_files():
        shared_crates |= set((module_config(name).get("wrap_dependencies", {}) or {}).keys())

    for crate in sorted(shared_crates):
        versions = {}
        for name in sorted(os.listdir(MODULES_ROOT)):
            if not os.path.exists(os.path.join(MODULES_ROOT, name, "config.yaml")):
                continue
            declared = (module_config(name).get("dependencies", {}) or {}).get(crate)
            if declared is not None:
                versions[name] = declared
        assert len(set(map(repr, versions.values()))) <= 1, \
            f"'{crate}' объявлен разными версиями: {versions}"
