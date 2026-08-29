"""Потолки памяти тайлера против лимита инстанса, который назначает хост.

Плоскость NetCDF разворачивается в линейную память wasm-инстанса, и потолок ей
(`PLANE_BUDGET` в `adapters/netcdf.rs`) выведен из того, сколько этой памяти
вообще дают: `INSTANCE_MEMORY_LIMIT` в ядре хоста. Величины эти живут по разные
стороны провода — одна в Rust-модуле, другая в Rust-хосте, — и связывает их
только арифметика в голове у того, кто их назначал.

Разъехаться им ничто не мешает: понизят лимит в `veldcore`, и модуль продолжит
считать, что ему дают гигабайт. Компилятор этого не поймает — для него это два
независимых числа, — а рантайм поймает трапом посреди чтения, то есть ровно тем,
что потолок и обязан предотвращать. У модуля есть свой тест на ту же пару
(`потолок_величины_укладывается_в_инстанс`), но он держит копию лимита у себя, а
копия — это и есть то, что разъезжается.
"""
import os
import re

from conftest import PROJECT_ROOT

HOST = os.path.join(PROJECT_ROOT, "veldcore", "platform", "host", "core", "src", "lib.rs")
NETCDF = os.path.join(PROJECT_ROOT, "veldmodules", "image-tiler", "src", "adapters", "netcdf.rs")

# Сколько обязано остаться сверх плоскости: пирамида, кэш метаданных, сам код
# модуля и провисание аллокатора.
HEADROOM = 128 * 1024 * 1024


def read(path: str) -> str:
    with open(path, encoding="utf-8") as f:
        return f.read()


def literal(source: str, name: str) -> int:
    """Значение константы вида `const NAME: u64 = 832 * 1024 * 1024;`."""
    found = re.search(rf"const {name}: u\w+ = ([^;]+);", source)
    assert found, f"константы {name} нет — её переименовали или убрали"
    body = found.group(1).replace("_", "")
    assert re.fullmatch(r"[\d\s*+]+", body), f"{name} задана не арифметикой чисел: {body}"
    return eval(body)  # noqa: S307 — выражение уже сведено к цифрам и знакам


def instance_limit() -> int:
    return literal(read(HOST), "INSTANCE_MEMORY_LIMIT")


def plane_budget() -> int:
    return literal(read(NETCDF), "PLANE_BUDGET")


def test_плоскость_укладывается_в_лимит_инстанса():
    limit, plane = instance_limit(), plane_budget()
    assert plane < limit, (
        "PLANE_BUDGET %d байт против лимита инстанса %d: плоскость не помещается"
        % (plane, limit)
    )
    assert limit - plane >= HEADROOM, (
        "сверх плоскости остаётся %d байт при нужных %d — на пирамиду, кэш "
        "метаданных и сам код не хватит" % (limit - plane, HEADROOM)
    )


def test_потолок_терпения_ниже_потолка_памяти():
    """По сети терпят меньше, чем помещается: иначе сетевой потолок мёртв."""
    source = read(NETCDF)
    wire, plane = literal(source, "WIRE_PLANE"), plane_budget()
    assert wire < plane, (
        "WIRE_PLANE %d не ниже PLANE_BUDGET %d — сетевой потолок не сработает "
        "никогда" % (wire, plane)
    )
