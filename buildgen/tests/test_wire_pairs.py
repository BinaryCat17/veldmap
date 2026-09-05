"""Величины, разъехавшиеся по разные стороны провода, сводятся здесь.

Провод — граница между хостом и wasm-модулем: константу одной стороны другая
не видит, компилятор такую пару не свяжет, а рантайм свяжет молча — щелчок
колеса другого размера, файл с другим суффиксом, потолок памяти не по лимиту.
Первый выбор — общая константа через wrap-крейт и `#[path]` (так сведён шаг
колеса у шара и канвы, `veldmodules/globe/src/wheel.rs`, и уровни лога у SDK и
ядра); сюда попадает лишь то, что через крейт не провести — по разные стороны
хоста.

Одна таблица и один парсер: пара — это два файла и два имени, значения
читаются из объявлений `const` и сравниваются числом либо строкой. Красный
называет обе половины: правящий одну обязан увидеть вторую.
"""
import ast
import os
import re

import pytest

from conftest import PROJECT_ROOT

PAIRS = [
    ("щелчок колеса в единицах окна",
     ("veldcore/platform/host/runners/desktop/src/main.rs", "WHEEL_NOTCH"),
     ("veldmodules/ui-service/src/handlers.rs", "RAW_WHEEL_NOTCH")),
    ("суффикс недокачанного файла",
     ("veldcore/platform/host/modules/network/src/download.rs", "PART_SUFFIX"),
     ("veldmodules/data-library/src/storage.rs", "PART_SUFFIX")),
    ("лимит линейной памяти инстанса",
     ("veldcore/platform/host/core/src/lib.rs", "INSTANCE_MEMORY_LIMIT"),
     ("veldmodules/image-tiler/src/budget.rs", "INSTANCE")),
    ("блок сети — проба JPEG 2000",
     ("veldcore/platform/host/modules/network/src/range.rs", "BLOCK"),
     ("veldmodules/image-tiler/src/adapters/excerpt.rs", "PROBE")),
    ("блок сети — край отпечатка",
     ("veldcore/platform/host/modules/network/src/range.rs", "BLOCK"),
     ("veldmodules/image-tiler/src/fingerprint.rs", "SAMPLE")),
]

NUMBER = re.compile(r"^[\d\s.*/+\-()_]+$")


def declaration(name: str) -> re.Pattern:
    """Объявление, а не упоминание: с начала строки, чтобы комментарий с тем
    же текстом раньше объявления не прочитался вместо него."""
    return re.compile(rf"^\s*(?:pub(?:\([^)]*\))?\s+)?const {name}\s*:[^=]+=\s*([^;]+);", re.M)


def declared(path: str, name: str):
    """Значение `const NAME: T = ...;` — число либо строка; `None` — объявления
    нет.

    Число берётся значением, а не текстом: `0.8` и `4 / 5` — один шаг, и
    проверка, придирающаяся к записи, ловила бы правку там, где её нет.
    """
    with open(os.path.join(PROJECT_ROOT, path), encoding="utf-8") as f:
        found = declaration(name).search(f.read())
    if not found:
        return None
    written = " ".join(found.group(1).split())
    if written.startswith('"'):
        assert written.endswith('"') and "\\" not in written, f"{path}: {name} — строка не буквальная: {written}"
        return written[1:-1]
    assert NUMBER.match(written), f"{path}: {name} задана не числом и не арифметикой чисел: {written}"
    return evaluate(written.replace("_", ""))


def evaluate(expression: str):
    """Арифметика чисел без `eval`: только константы и четыре действия."""
    def walk(node):
        if isinstance(node, ast.Expression):
            return walk(node.body)
        if isinstance(node, ast.Constant) and isinstance(node.value, (int, float)):
            return node.value
        if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.USub):
            return -walk(node.operand)
        if isinstance(node, ast.BinOp):
            left, right = walk(node.left), walk(node.right)
            if isinstance(node.op, ast.Add):
                return left + right
            if isinstance(node.op, ast.Sub):
                return left - right
            if isinstance(node.op, ast.Mult):
                return left * right
            if isinstance(node.op, ast.Div):
                return left / right
        raise AssertionError(f"в константе не арифметика: {ast.dump(node)}")
    return walk(ast.parse(expression, mode="eval"))


@pytest.mark.parametrize("what,host,module", PAIRS, ids=[p[0] for p in PAIRS])
def test_both_sides_of_the_wire_agree(what, host, module):
    left, right = declared(*host), declared(*module)
    both = f"{host[0]}::{host[1]} и {module[0]}::{module[1]}"
    assert left is not None and right is not None, (
        f"{what}: одной половины пары нет — переименовали или убрали; пара это {both}"
    )
    assert left == right, (
        f"{what}: {host[0]}::{host[1]} = {left!r}, а {module[0]}::{module[1]} = {right!r} — "
        "это одна величина по разные стороны провода, править надо обе"
    )


def test_the_parser_reads_declarations_only(tmp_path):
    """Ослабевший парсер сравнивал бы пустое с пустым — или комментарий с
    объявлением."""
    source = tmp_path / "x.rs"
    source.write_text(
        "/// Пример в комментарии: `const A: u64 = 999;` — не объявление.\n"
        "pub(crate) const A: u64 = 4 * 1_024;\n"
        "const B: &str = \".part\";\n"
        "const C: f32 =\n    1.0 / 4.0;\n",
        encoding="utf-8")
    assert declared(str(source), "A") == 4096
    assert declared(str(source), "B") == ".part"
    assert declared(str(source), "C") == 0.25
    assert declared(str(source), "D") is None
