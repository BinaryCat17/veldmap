"""Список сжатий TIFF в коде и набор декодеров, собранный в крейт.

Читать чанк умеет не всякий TIFF, а тот, чьё сжатие разбирает собранный у нас
декодер. Какие разбираются — решает `config.yaml`: фичи крейта `tiff` тянут
каждая свою библиотеку. Какие мы объявляем читаемыми — решает таблица
`DECODED` в `tiff.rs`, и по ней приходит отказ при описании.

Мест, стало быть, два, и разъехаться им ничто не мешает: уберут фичу из
`config.yaml` — таблица останется обещать сжатие, которого декодер уже не
знает, и отказ переедет обратно на первый чанк, то есть на скачанные впустую
гигабайты. Обратная сторона тише и хуже: добавят фичу — таблица погасит
читаемый файл, и человек увидит «не разбирается» над тем, что открылось бы.

Компилятору обе стороны безразличны: для него это число и строка. Здесь они и
сводятся.
"""
import os
import re

import yaml

from conftest import PROJECT_ROOT

TIFF_RS = os.path.join(PROJECT_ROOT, "veldmodules", "image-tiler", "src", "adapters", "tiff.rs")
CONFIG = os.path.join(PROJECT_ROOT, "veldmodules", "image-tiler", "config.yaml")

# Версия, под которую снят список ниже. Сверяется отдельно: подними её в
# config.yaml — и умолчания с набором фич могут стать другими, а тест останется
# зелёным поверх устаревшего знания, то есть перестанет ловить ровно то, ради
# чего заведён. Пусть лучше покраснеет и заставит перечитать чужой Cargo.toml.
TIFF_VERSION = "0.11"

# Умолчания крейта tiff 0.11 (его Cargo.toml, секция [features]).
TIFF_DEFAULTS = {"deflate", "fax", "jpeg", "lzw"}

# Коды сжатия, которые открывает каждая фича. Значения — из tags.rs крейта;
# `None` и `PackBits` не стоят ни одной зависимости и есть всегда.
ALWAYS = {1, 0x8005}
BY_FEATURE = {
    "deflate": {8, 0x80B2},
    "fax": {4},  # только четвёртая группа: Fax3 крейт не декодирует вовсе
    "jpeg": {7},
    "lzw": {5},
    "zstd": {0xC350},
    "webp": {0xC351},
}


def read(path: str) -> str:
    with open(path, encoding="utf-8") as f:
        return f.read()


def declared() -> set[int]:
    """Коды из таблицы `DECODED`."""
    source = read(TIFF_RS)
    at = source.index("const DECODED")
    table = source[at:source.index("];", at)]
    return {int(code, 0) for code in re.findall(r"\(\s*(0x[0-9A-Fa-f]+|\d+)\s*,", table)}


def built() -> set[int]:
    """Коды, которые открывают фичи крейта из config.yaml."""
    tiff = yaml.safe_load(read(CONFIG))["rust"]["dependencies"]["tiff"]
    features = set(tiff.get("features") or ())
    if tiff.get("default-features", True):
        features |= TIFF_DEFAULTS
    codes = set(ALWAYS)
    for feature in features:
        codes |= BY_FEATURE.get(feature, set())
    return codes


def test_таблица_сжатий_сходится_с_фичами_крейта():
    assert declared() == built(), (
        "DECODED в tiff.rs и фичи крейта tiff в config.yaml разошлись: "
        "лишние в таблице %s, недостающие %s"
        % (sorted(declared() - built()), sorted(built() - declared()))
    )


def test_список_снят_с_той_версии_крейта_что_в_конфиге():
    tiff = yaml.safe_load(read(CONFIG))["rust"]["dependencies"]["tiff"]
    version = tiff["version"] if isinstance(tiff, dict) else tiff
    assert str(version).startswith(TIFF_VERSION), (
        "крейт tiff в config.yaml версии %s, а список фич снят с %s — "
        "перечитайте его Cargo.toml и поправьте TIFF_DEFAULTS/BY_FEATURE"
        % (version, TIFF_VERSION)
    )


def test_таблица_сжатий_не_пуста_и_знает_несжатый():
    codes = declared()
    assert 1 in codes, "несжатый TIFF обязан читаться при любом наборе фич"
    assert len(codes) > 1, "таблица сжатий пуста — отказ погасит вообще всё"
