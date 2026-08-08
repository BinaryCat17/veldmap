"""Регрессии на живых схемах проекта.

Отдельно от тестов правил: те описывают, каким валидатор обязан быть, а эти —
каким на сегодня получается проект. Падение здесь означает не сломанный
генератор, а изменившийся контракт, и увидеть его надо в момент правки схемы,
а не в рантайме.
"""
import pytest

from conftest import PROTO_DIR


def module_schemas(real_schemas):
    return [s for s in real_schemas if s[3] == "module"]


def platform_schemas(real_schemas):
    return [s for s in real_schemas if s[3] == "platform"]


def test_every_schema_validates(gen, real_schemas, real_universe):
    """Ни одна схема проекта не должна давать ошибок валидации."""
    failures = {}
    for name, path, schema, kind in real_schemas:
        if kind == "module":
            import os
            errors, _ = gen.validate_module_schema(
                schema, os.path.dirname(path), PROTO_DIR, real_universe)
        else:
            errors = gen.validate_core_schema(schema, real_universe)
        errors += gen.validate_schema_identity(schema, path)
        if errors:
            failures[name] = errors
    assert failures == {}


def test_every_schema_has_an_unambiguous_terminal(gen, real_schemas):
    """У каждого запроса ровно один конец — иначе учёт задачи не закрыть."""
    failures = {name: errors
                for name, _, schema, _ in real_schemas
                if (errors := gen.terminal_reply_of(schema)[1])}
    assert failures == {}


def test_accounted_operations_are_exactly_the_declared_ones(gen, real_schemas):
    """Что именно хост учитывает как задачу.

    Список короткий намеренно: отменяемым объявляет себя тот, кому нечего
    терять при убийстве (у image-loader состояния между вызовами нет вовсе).
    Новая строка здесь — это новая убиваемая операция, и появиться она должна
    осознанно.
    """
    flow = []
    for name, _, schema, _ in real_schemas:
        entries, errors = gen.flow_entries(schema.get("name"), schema)
        assert errors == [], f"{name}: {errors}"
        flow.extend(entries)

    assert sorted((e["request"], e["terminal"]) for e in flow) == [
        ("image-loader/on_load",   "image-loader/on_load_result"),
        ("network/on_fs_download", "network/on_fs_download_result"),
        ("network/on_http",        "network/on_http_result"),
    ]


def test_download_progress_is_not_terminal(gen, real_schemas):
    """Прогресс приходит многократно и операцию не закрывает: сделай его
    терминальным — и хост снимет учёт на первом же присланном байте."""
    network = next(s for name, _, s, _ in real_schemas if name == "network")
    terminal, errors = gen.terminal_reply_of(network)
    assert errors == []
    assert terminal["on_fs_download"] == "on_fs_download_result"


def test_targeted_topics_are_declared_deliberately(gen, real_schemas):
    """Адресная доставка — исключение из широковещательной шины, и список
    исключений должен быть виден целиком."""
    targeted = set()
    for _, _, schema, _ in real_schemas:
        name = schema.get("name")
        for entry in gen.topic_entries(name, schema, "outputs", lambda k, n, d: "X"):
            if entry["targeted"]:
                targeted.add(f"{name}/{entry['name']}")
    assert targeted == {"app/on_window_resized", "ui-service/on_ui_event"}
