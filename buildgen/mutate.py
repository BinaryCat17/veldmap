#!/usr/bin/env python3
"""Мутационная проверка тестов: код ломается названным способом, и тест обязан
покраснеть. Зелёный на сломанном коде — это тест, который ничего не держит.

Запускается вручную, не сборкой: одна мутация — это `cargo test` крейта, то
есть секунды на кэшированной цели и минуты на холодной.

    buildgen/.venv/bin/python buildgen/mutate.py image-tiler          # все мутации крейта
    buildgen/.venv/bin/python buildgen/mutate.py image-tiler хвост    # по подстроке имени

Мутации лежат в `buildgen/mutations/<крейт>.txt` блоками из четырёх строк —
`имя:`, `файл:` (от корня репозитория), `было:`, `стало:` — разделёнными
пустой строкой; строки с `#` — комментарий. Крейт называется именем модуля из
veldmodules/, `<модуль>-wrap` для его wrap-крейта либо именем пакета
воркспейса veldcore (`veldmap-host-core`).

Фрагмент `было:` обязан встретиться в файле ровно один раз — иначе мутация
не применяется, называется негодной и считается наравне с выжившей. Исходник
восстанавливается всегда, и по Ctrl-C тоже. Код возврата — число выживших и
негодных мутаций.
"""
import os
import subprocess
import sys

BUILDGEN_DIR = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, BUILDGEN_DIR)
import build  # noqa: E402 — пути крейтов знает сборка, второго списка не нужно

MUTATIONS_DIR = os.path.join(BUILDGEN_DIR, "mutations")
KEYS = ("имя", "файл", "было", "стало")


def load(crate: str) -> list[dict]:
    path = os.path.join(MUTATIONS_DIR, f"{crate}.txt")
    with open(path, encoding="utf-8") as f:
        lines = [line.rstrip("\n") for line in f if not line.startswith("#")]
    found, block = [], {}
    for line in lines + [""]:
        if not line.strip():
            if block:
                missing = [key for key in KEYS if key not in block]
                assert not missing, f"{path}: у мутации {block.get('имя', '?')!r} нет {missing}"
                found.append(block)
                block = {}
            continue
        key, _, value = line.partition(":")
        assert key in KEYS, f"{path}: непонятная строка {line!r}"
        block[key] = value.strip()
    return found


def cargo_test(crate: str) -> tuple[list[str], str]:
    """Команда и каталог теста крейта — те же, что у build.py."""
    modules = {m["name"]: m for m in build.discover_modules()}
    if crate in modules:
        return (["cargo", "test", "--release", "-q", "-p", modules[crate]["package"]],
                os.path.join(modules[crate]["dir"], "generated"))
    if crate.endswith("-wrap") and crate[:-5] in modules:
        return (["cargo", "test", "--release", "-q"],
                os.path.join(modules[crate[:-5]]["dir"], "generated", "wraps", "rust"))
    return (["cargo", "test", "--release", "-q", "-p", crate],
            os.path.join(build.PROJECT_ROOT, "veldcore"))


def write(path: str, text: str) -> None:
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(__doc__)
        return 2
    crate, needle = argv[1], (argv[2] if len(argv) > 2 else "")
    cmd, cwd = cargo_test(crate)
    bad = 0
    for mutation in load(crate):
        if needle not in mutation["имя"]:
            continue
        path = os.path.join(build.PROJECT_ROOT, mutation["файл"])
        with open(path, encoding="utf-8") as f:
            original = f.read()
        count = original.count(mutation["было"])
        if count != 1:
            bad += 1
            print(f"НЕГОДНА {mutation['имя']} — фрагмент встречается {count} раз")
            continue
        # Запись мутации — уже под `finally`: прерванный между записью и
        # прогоном оставил бы дерево сломанным.
        try:
            write(path, original.replace(mutation["было"], mutation["стало"]))
            red = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True).returncode != 0
        finally:
            write(path, original)
        if red:
            print(f"OK    {mutation['имя']} — тест покраснел")
        else:
            bad += 1
            print(f"ПЛОХО {mutation['имя']} — тест прошёл на сломанном коде")
    print(f"исходник восстановлен; выжило или негодно мутаций: {bad}")
    return bad


if __name__ == "__main__":
    sys.exit(main(sys.argv))
