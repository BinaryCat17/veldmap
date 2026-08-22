"""Совет в отказе провайдера и имена, которые он называет.

Ключей Copernicus нет — и `data-provider` отвечает на всякий ход в хранилище
не кодом 403, а тем, чего не хватает и как это исправить (`cdse::NO_KEYS`).
Совет этот называет две переменные окружения поимённо, а подставляет их в
конфиг хост, читая `${VAR}` из `.env`.

Мест, стало быть, три: конфиг, пример `.env` и текст совета. Разъехаться им
ничто не мешает — переименуют переменную в конфиге, и совет начнёт посылать
заводить несуществующую, — а сборка от этого не покраснеет: для неё это просто
строки. Здесь они и сводятся.
"""
import json
import os
import re

from conftest import PROJECT_ROOT

CDSE = os.path.join(PROJECT_ROOT, "veldmodules", "data-provider", "src", "cdse.rs")
CONFIG = os.path.join(PROJECT_ROOT, "runtime", "config", "data-provider.json")
EXAMPLE = os.path.join(PROJECT_ROOT, ".env.example")


def read(path: str) -> str:
    with open(path, encoding="utf-8") as f:
        return f.read()


def advice() -> str:
    """Текст `NO_KEYS` — от объявления до закрывающей кавычки."""
    source = read(CDSE)
    at = source.index("pub const NO_KEYS")
    text = source[at:source.index(";", at)]
    # Строка сложена продолжением строки (`\` в конце): склеиваем как rustc.
    return re.sub(r"\\\s*\n\s*", "", text)


def config_vars() -> set[str]:
    """Переменные, которые конфиг просит подставить."""
    return set(re.findall(r"\$\{([A-Za-z0-9_]+)\}", read(CONFIG)))


def test_the_advice_names_the_variables_the_config_asks_for():
    said = advice()
    asked = config_vars()

    assert asked, f"{CONFIG} больше не просит подставить ничего — совет устарел"
    for name in asked:
        assert name in said, f"'{name}' стои́т в конфиге, но отказ о ней молчит"


def test_the_example_declares_the_same_variables():
    # Совет велит скопировать пример; не окажись в нём этих строк — копировать
    # было бы нечего, а человек искал бы их сам.
    example = read(EXAMPLE)
    for name in config_vars():
        assert f"{name}=" in example, f"'{name}' нет в .env.example"


def test_the_advice_points_at_a_file_that_exists():
    said = advice()
    assert ".env.example" in said, "совет не называет, откуда брать образец"
    assert os.path.exists(EXAMPLE), ".env.example пропал, а отказ на него ссылается"


def test_the_config_keeps_the_secrets_out_of_git():
    # Ключ, вписанный в конфиг вместо `${VAR}`, уехал бы в репозиторий: конфиги
    # в git, а `.env` — нет. Отказ при этом молчал бы: ключи-то есть.
    #
    # Проверяются поля поимённо, а не значения по виду: вписанный ключ выглядит
    # обычной строкой, и всякий отбор «похоже на секрет» пропустил бы ровно ту
    # беду, ради которой проверка заведена.
    config = json.loads(read(CONFIG))
    for field in ("access_key", "secret_key"):
        value = config.get(field)
        assert isinstance(value, str) and re.fullmatch(r"\$\{[A-Za-z0-9_]+\}", value), \
            f"'{field}' обязано быть подстановкой из окружения, а не значением: {value!r}"
