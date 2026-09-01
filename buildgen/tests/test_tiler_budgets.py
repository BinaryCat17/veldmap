"""Учёт памяти тайлера против лимита инстанса, который назначает хост.

Всё, что тайлер разворачивает — плоскость величины, полосы каскада, кадр
декодера, — ложится в линейную память wasm-инстанса, а сколько её дают, знает
ядро хоста: `INSTANCE_MEMORY_LIMIT`. Модуль держит это число своей копией
(`budget::INSTANCE`), и копия она поневоле: величины лежат по разные стороны
провода, хостовой константы модулю не видно.

Разъехаться им ничто не мешает: понизят лимит в `veldcore`, и модуль продолжит
считать, что ему дают гигабайт. Компилятор этого не поймает — для него это два
независимых числа, — а рантайм поймает трапом посреди чтения, то есть ровно тем,
что потолок и обязан предотвращать.

Прочие потолки тайлера сюда не поднимаются: они назначены числами, и назначены
против того же гигабайта — то есть проверять их против него же значило бы
сверять число с самим собой. Складывает их между собой тот, кто их тратит
(`netcdf::affordable` — единственный, кто сегодня складывает), и накрыто это
юнит-тестами модуля. Здесь остаётся только то, чего юнит-тест увидеть не
может, — число из чужого крейта.
"""
import os
import re

from conftest import PROJECT_ROOT

HOST = os.path.join(PROJECT_ROOT, "veldcore", "platform", "host", "core", "src", "lib.rs")
BUDGET = os.path.join(PROJECT_ROOT, "veldmodules", "image-tiler", "src", "budget.rs")
NETCDF = os.path.join(PROJECT_ROOT, "veldmodules", "image-tiler", "src", "adapters", "netcdf.rs")


def read(path: str) -> str:
    with open(path, encoding="utf-8") as f:
        return f.read()


def literal(source: str, name: str) -> int:
    """Значение константы вида `pub const NAME: u64 = 832 * 1024 * 1024;`."""
    found = re.search(rf"(?:pub )?const {name}: u\w+ = ([^;]+);", source)
    assert found, f"константы {name} нет — её переименовали или убрали"
    body = found.group(1).replace("_", "")
    assert re.fullmatch(r"[\d\s*+-]+", body), f"{name} задана не арифметикой чисел: {body}"
    return eval(body)  # noqa: S307 — выражение уже сведено к цифрам и знакам


def test_модуль_знает_настоящий_лимит_инстанса():
    host = literal(read(HOST), "INSTANCE_MEMORY_LIMIT")
    module = literal(read(BUDGET), "INSTANCE")
    assert module == host, (
        "тайлер считает, что ему дают %d байт, а хост даёт %d — все его потолки "
        "выведены из первого числа и промахнутся на разнице" % (module, host)
    )


def test_запас_оставляет_место_работе():
    """Иначе свободного не останется и всякий источник получит отказ."""
    source = read(BUDGET)
    instance, reserve = literal(source, "INSTANCE"), literal(source, "RESERVE")
    assert 0 < reserve < instance // 2, (
        "запас %d байт при лимите %d — либо нулевой, либо съел половину"
        % (reserve, instance)
    )


def test_потолок_терпения_ниже_потолка_памяти():
    """По сети терпят меньше, чем помещается: иначе сетевой потолок мёртв."""
    budget = read(BUDGET)
    free = literal(budget, "INSTANCE") - literal(budget, "RESERVE")
    wire = literal(read(NETCDF), "WIRE_PLANE")
    assert wire < free, (
        "WIRE_PLANE %d не ниже свободной памяти %d — сетевой потолок не "
        "сработает никогда" % (wire, free)
    )
