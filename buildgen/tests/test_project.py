"""Регрессии на живых схемах проекта.

Отдельно от тестов правил: те описывают, каким валидатор обязан быть, а эти —
каким на сегодня получается проект. Падение здесь означает не сломанный
генератор, а изменившийся контракт, и увидеть его надо в момент правки схемы,
а не в рантайме.
"""
import argparse
import re

import pytest

from conftest import BUILDGEN_DIR, PROTO_DIR


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


def test_killable_operations_are_exactly_the_declared_ones(gen, real_schemas):
    """Что именно хост позволяет убить.

    Список короткий намеренно: отменяемым объявляет себя тот, кому нечего
    терять при убийстве (у image-tiler состояния между вызовами нет вовсе).
    Новая строка здесь — это новая убиваемая операция, и появиться она должна
    осознанно.

    Учёт при этом заводится и на остальных запросах — им держится терминальный
    ответ за упавшего, — но убить их нельзя, и стаба убийства заказчику не
    достаётся (см. `test_every_answered_request_is_accounted`).
    """
    flow = []
    for name, _, schema, _ in real_schemas:
        entries, errors = gen.flow_entries(schema.get("name"), schema)
        assert errors == [], f"{name}: {errors}"
        flow.extend(entries)

    assert sorted((e["request"], e["terminal"]) for e in flow if e["cancellable"]) == [
        ("image-tiler/on_produce", "image-tiler/on_produce_done"),
        ("network/on_fs_download", "network/on_fs_download_result"),
        ("network/on_http",        "network/on_http_result"),
    ]


def test_every_answered_request_is_accounted(gen, real_schemas):
    """Учёт покрывает всякий запрос, у которого объявлен ответ.

    На этом и держится обещание «терминальный ответ приходит всегда»: упади
    исполнитель посреди работы — конец операции договорит хост, а договорить он
    может только учтённое. Пропусти таблица хоть один такой запрос, и его
    заказчик после трапа ждал бы ответа до конца процесса.

    Считается по схемам, а не по списку: список устарел бы на первом же новом
    топике, а правило — нет.
    """
    for name, _, schema, _ in real_schemas:
        accounted = {e["request"] for e in gen.flow_entries(schema.get("name"), schema)[0]}
        answered = {f"{schema.get('name')}/{request}"
                    for request in gen.terminal_reply_of(schema)[0]}
        assert answered == accounted, f"{name}: без учёта остались {answered - accounted}"


def test_the_flow_table_reaches_the_host(gen, real_schemas, tmp_path):
    """Учёт держится на таблице, которую видит ХОСТ.

    Тесты выше спрашивают `flow_entries` — то, что считает валидатор. Между ним
    и хостом лежат ещё два места, где запись может потеряться молча: отбор при
    сборке `template_data` и цикл в шаблоне. Ослабей любое из них, и сборка
    пройдёт зелёной, схемы сойдутся, а заказчик после трапа исполнителя будет
    ждать ответа до конца процесса — то есть ровно тот класс ошибки, ради
    которого тесты buildgen и заведены.

    Поэтому здесь генерация зовётся целиком и проверяется отрисованное.
    """
    args = argparse.Namespace(host_bindings=str(tmp_path), proto_dir=PROTO_DIR, package=None)
    gen.generate_host_bindings(args, BUILDGEN_DIR)

    rendered = (tmp_path / "src" / "lib.rs").read_text(encoding="utf-8")
    at = rendered.index("pub const FLOW")
    table = rendered[at:rendered.index("];", at)]

    for name, _, schema, _ in real_schemas:
        service = schema.get("name")
        killable = {e["request"] for e in gen.flow_entries(service, schema)[0] if e["cancellable"]}
        for request, terminal in gen.terminal_reply_of(schema)[0].items():
            topic = f"{service}/{request}"
            row = f'("{topic}", "{service}/{terminal}", {str(topic in killable).lower()})'
            assert row in table, f"{name}: в таблице хоста нет строки {row}"

    # Хост ищет в таблице бинарным поиском (`flow::exchange_of`), а тот верен
    # только над отсортированным: перестань генератор сортировать — и часть
    # запросов молча осталась бы без учёта. Порядок сравнивается в том виде, в
    # каком его сравнивает Rust, — по байтам строки.
    requests = [row[0] for row in
                re.findall(r'\("([^"]+)", "([^"]+)", (?:true|false)\)', table)]
    assert requests == sorted(requests, key=lambda s: s.encode("utf-8")), \
        "таблица FLOW не отсортирована по запросу — бинарный поиск хоста промахнётся"
    assert len(requests) == len(set(requests)), "в таблице FLOW запрос повторяется"


def test_download_progress_is_not_terminal(gen, real_schemas):
    """Прогресс приходит многократно и операцию не закрывает: сделай его
    терминальным — и хост снимет учёт на первом же присланном байте."""
    network = next(s for name, _, s, _ in real_schemas if name == "network")
    terminal, errors = gen.terminal_reply_of(network)
    assert errors == []
    assert terminal["on_fs_download"] == "on_fs_download_result"


def test_intermediate_replies_are_exactly_the_declared_ones(gen, real_schemas):
    """Ответы, за которыми по той же корреляции придёт ещё один.

    Список едет в подписчиков: SDK ловит по нему снятие запроса с учёта на
    прогрессе (`Correlator::take`, `Latest::settle`), которое иначе молча
    потеряло бы следующий ответ вместе с приехавшим в нём ресурсом. Пусто
    здесь — значит предупреждать не о чем ни в одном модуле.
    """
    intermediate = {f"{schema.get('name')}/{topic}"
                    for _, _, schema, _ in real_schemas
                    for topic in gen.intermediate_replies_of(schema)}

    assert sorted(intermediate) == [
        "image-tiler/on_produce_progress",
        "image-tiler/on_produced",
        "network/on_fs_download_progress",
        "tile-cache/on_tile",
    ]


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
