"""Тесты валидатора схем.

Схема — единственный источник истины о топиках, и единственный, кто эту истину
проверяет, — `generate.py`. Ломается он молча: ослабевшая проверка не мешает
сборке пройти, а расхождение всплывает уже в рантайме недоставленным событием.
Здесь зафиксировано, что именно он обязан считать ошибкой.

Отдельно закреплён поток операции (`replies_to` / `terminal` / `cancellable`):
на нём держится учёт задач в хосте — `Dispatcher::account` открывает учёт по
запросу и закрывает по его терминальному ответу, а имена этих топиков берёт из
таблиц, которые считает `flow_entries`.
"""
import pytest

from conftest import PROTO_DIR, make_universe


# ── Пары запрос/ответ ────────────────────────────────────────────────────────
#
# `replies_to` — единственное, что делает топик коррелированным: только у таких
# стабы принимают correlation_id аргументом.

def test_request_maps_to_all_its_replies(gen):
    schema = {"interface": {
        "inputs": {"on_download": {"type": "fs/FsReadRequest"}},
        "outputs": {
            "on_progress": {"type": "fs/FsReadResult", "replies_to": "on_download"},
            "on_result":   {"type": "fs/FsReadResult", "replies_to": "on_download",
                            "terminal": True},
        }}}
    requests, replies = gen.correlated_topics(schema)
    assert sorted(requests["on_download"]) == ["on_progress", "on_result"]
    assert replies == {"on_progress", "on_result"}


def test_topic_without_replies_to_is_not_correlated(gen):
    # У события «состояние изменилось» корреспондента нет, и приписать ему
    # корреляцию нельзя по построению — стаб её не примет.
    schema = {"interface": {"outputs": {"on_state": {"type": "fs/FsReadResult"}}}}
    requests, replies = gen.correlated_topics(schema)
    assert requests == {}
    assert replies == set()


# ── Терминальный ответ ───────────────────────────────────────────────────────
#
# Конец операции у запроса ровно один: по нему хост снимает учёт задачи и его
# же публикует за убитого исполнителя.

def reply(request, terminal=False):
    entry = {"type": "fs/FsReadResult", "replies_to": request}
    if terminal:
        entry["terminal"] = True
    return entry


def schema_with(replies, cancellable=False, name="svc"):
    """Схема об одном запросе `on_work` и заданных ответах на него."""
    request = {"type": "fs/FsReadRequest"}
    if cancellable:
        request["cancellable"] = True
    return {"name": name,
            "interface": {"inputs": {"on_work": request}, "outputs": replies}}


def test_single_reply_is_terminal_by_default(gen):
    # Писать `terminal: true` там, где выбора нет, значило бы повторять
    # очевидное — валидатор выводит это сам.
    terminal, errors = gen.terminal_reply_of(schema_with({"on_done": reply("on_work")}))
    assert errors == []
    assert terminal == {"on_work": "on_done"}


def test_marked_reply_wins_when_there_are_several(gen):
    # Ровно случай network/on_fs_download: прогресс операцию не закрывает.
    terminal, errors = gen.terminal_reply_of(schema_with({
        "on_progress": reply("on_work"),
        "on_done":     reply("on_work", terminal=True)}))
    assert errors == []
    assert terminal == {"on_work": "on_done"}


@pytest.mark.parametrize("replies, expected", [
    pytest.param({"on_a": reply("on_work"), "on_b": reply("on_work")},
                 "ровно один",
                 id="несколько ответов без отметки"),
    pytest.param({"on_a": reply("on_work", terminal=True),
                  "on_b": reply("on_work", terminal=True)},
                 "терминальный ответ у запроса один",
                 id="две отметки на одном запросе"),
])
def test_ambiguous_terminal_is_rejected(gen, replies, expected):
    # Угадывать конец операции валидатор не вправе: ошибка здесь означала бы
    # либо учёт, снятый на первом же прогрессе, либо два «всё кончилось».
    terminal, errors = gen.terminal_reply_of(schema_with(replies))
    assert terminal == {}
    assert len(errors) == 1
    assert expected in errors[0]


# ── Промежуточные ответы ─────────────────────────────────────────────────────
#
# Дополнение терминального ответа внутри пары, и считается оно ради заказчика:
# сняв запрос с учёта на прогрессе, он перестанет опознавать то, что придёт по
# той же корреляции следом. Список едет в модуль, где SDK ловит такое снятие.

def test_progress_is_intermediate_and_terminal_is_not(gen):
    # Ровно случай network/on_fs_download: прогресс приходит многократно.
    assert gen.intermediate_replies_of(schema_with({
        "on_progress": reply("on_work"),
        "on_done":     reply("on_work", terminal=True)})) == {"on_progress"}


def test_single_reply_is_never_intermediate(gen):
    # Единственный ответ терминален по умолчанию — промежуточным ему стать
    # неоткуда, и предупреждать на нём не о чем.
    assert gen.intermediate_replies_of(schema_with({"on_done": reply("on_work")})) == set()


def test_topic_without_a_pair_is_not_intermediate(gen):
    # У события без `replies_to` корреспондента нет: снимать с учёта нечего.
    assert gen.intermediate_replies_of(
        {"name": "svc",
         "interface": {"outputs": {"on_state": {"type": "fs/FsReadResult"}}}}) == set()


# ── Таблица потока и отменяемость ────────────────────────────────────────────
#
# Учёт открывается на каждом запросе с объявленным ответом: им держится обещание
# «терминальный ответ приходит всегда» — упавший посреди работы исполнитель
# иначе оставил бы заказчика ждать вечно. `cancellable` — свойство записи, а не
# повод её завести: убить можно только объявленное отменяемым. Учёт, который
# нечем закрыть, — незакрываемая задача, поэтому пара «запрос/ответ»
# обязательна.

def test_a_request_yields_fully_qualified_topics(gen):
    # Таблица уезжает в хост как есть: в ней топики вида `<сервис>/<имя>`,
    # потому что диспетчер сравнивает именно их.
    schema = schema_with({"on_done": reply("on_work")}, cancellable=True,
                         name="worker")
    entries, errors = gen.flow_entries("worker", schema)
    assert errors == []
    assert entries == [{"request":     "worker/on_work",
                        "terminal":    "worker/on_done",
                        "cancellable": True}]


def test_a_plain_request_is_accounted_but_not_killable(gen):
    # Учёт нужен и неотменяемому: убивать у него нечего, а договорить конец за
    # упавшего исполнителя надо — заказчик ждёт ответа одинаково.
    entries, errors = gen.flow_entries("fs", schema_with({"on_done": reply("on_work")}))
    assert errors == []
    assert entries == [{"request":     "fs/on_work",
                        "terminal":    "fs/on_done",
                        "cancellable": False}]


def test_a_topic_without_a_reply_stays_out_of_the_table(gen):
    # Учёту не на чем держаться: закрыть его нечем, и запись висела бы вечно.
    # Так живут рассылки состояния и события — у них корреспондента нет вовсе.
    entries, errors = gen.flow_entries("svc", schema_with({}))
    assert errors == []
    assert entries == []


def test_cancellable_without_a_reply_is_rejected(gen):
    # Без ответа учёт открылся бы навсегда: закрыть его нечем, и заказчик
    # никогда не узнал бы, что работа кончилась.
    entries, errors = gen.flow_entries("svc", schema_with({}, cancellable=True))
    assert entries == []
    assert len(errors) == 1
    assert "`cancellable: true` требует ответа" in errors[0]


def test_cancellable_with_ambiguous_terminal_reports_once(gen):
    # Про неоднозначный конец уже сказано в terminal_reply_of; второй раз о том
    # же, да ещё другими словами, только путал бы.
    entries, errors = gen.flow_entries("svc", schema_with(
        {"on_a": reply("on_work"), "on_b": reply("on_work")}, cancellable=True))
    assert entries == []
    assert len(errors) == 1
    assert "ровно один" in errors[0]


def test_flow_errors_surface_through_schema_validation(core_errors):
    # flow_entries зовётся из валидатора — ошибка потока обязана валить сборку,
    # а не всплывать позже при генерации биндингов хоста.
    errors = core_errors(schema_with({}, cancellable=True))
    assert any("cancellable" in e for e in errors), errors


# ── Проверка типов ───────────────────────────────────────────────────────────
#
# Тип топика объявлен ровно один раз и обязан существовать: иначе стаб
# сгенерировался бы на несуществующий Rust-тип.

def test_valid_schema_has_no_errors(core_errors):
    assert core_errors({"name": "fs", "interface": {
        "inputs":  {"on_read": {"type": "fs/FsReadRequest"}},
        "outputs": {"on_read_result": {"type": "core/ResourceOpened",
                                       "replies_to": "on_read"}}}}) == []


@pytest.mark.parametrize("type_str, expected", [
    pytest.param("nosuch/Message",     "unknown proto package", id="нет такого пакета"),
    pytest.param("core/NoSuchMessage", "not found in package",  id="нет такого сообщения"),
    pytest.param("ResourceOpened",     "malformed type",        id="без пакета"),
    pytest.param("module/Something",   "has no types.proto",    id="module/ без types.proto"),
])
def test_bad_type_reference_is_rejected(core_errors, type_str, expected):
    errors = core_errors({"name": "svc",
                          "interface": {"inputs": {"on_x": {"type": type_str}}}})
    assert any(expected in e for e in errors), errors


def test_platform_service_may_declare_a_payload_free_topic(core_errors):
    # app/on_ready — сигнал без данных; это отличие диалекта платформенного
    # сервиса от wasm-модуля, и оно намеренное.
    assert core_errors({"name": "app", "interface": {"outputs": {"on_ready": {}}}}) == []


def test_wasm_module_must_declare_a_payload(gen, universe):
    schema = {"name": "svc", "interface": {"outputs": {"on_ready": {}}}}
    errors, _ = gen.validate_service_schema(schema, universe, allow_empty_payload=False)
    assert any("missing 'type'" in e for e in errors), errors


def test_replies_to_must_name_an_existing_input(core_errors):
    # Опечатка здесь означала бы ответ на запрос, которого нет: пара не
    # сложилась бы, а корреляция уехала бы в пустоту.
    errors = core_errors({"name": "svc", "interface": {
        "inputs":  {"on_read": {"type": "fs/FsReadRequest"}},
        "outputs": {"on_read_result": {"type": "core/ResourceOpened",
                                       "replies_to": "on_raed"}}}})
    assert any("is not one of this service's interface.inputs" in e for e in errors), errors


def test_own_package_must_be_written_as_module(gen, tmp_path):
    # Своё имя пакета кодоген разворачивает в тот же alias, что и `module/`, —
    # два написания одного значат, что читающий схему обязан знать оба. Хуже
    # того, развёрнутое неотличимо от чужого, а чужое требует объявленной
    # зависимости: одно и то же на вид, разное по правилам.
    uni = make_universe(core=(("ResourceOpened",), "core"),
                        mine=(("Thing",), "svc"))
    module_dir = tmp_path / "svc"
    module_dir.mkdir()
    (module_dir / "types.proto").write_text("package veldmap.mine;\n")

    spelled = {"name": "svc", "interface": {"inputs": {"on_x": {"type": "mine/Thing"}}}}
    errors, _ = gen.validate_module_schema(spelled, str(module_dir), PROTO_DIR, uni)
    assert any("names this module's own package" in e for e in errors), errors

    short = {"name": "svc", "interface": {"inputs": {"on_x": {"type": "module/Thing"}}}}
    errors, resolved = gen.validate_module_schema(short, str(module_dir), PROTO_DIR, uni)
    assert errors == [], errors
    assert resolved["inputs"]["on_x"] == "mine/Thing"


def test_the_own_package_rule_spares_a_dependency_schema(gen, tmp_path):
    # Тем же `check` проверяются типы, взятые из схемы зависимости, а там
    # `module/` значит её пакет. При взаимной зависимости чужой alias совпадёт с
    # нашим — и правило, сработав, посоветовало бы написать в чужой схеме
    # `module/`, то есть указать не на тот пакет.
    uni = make_universe(core=(("ResourceOpened",), "core"),
                        alpha=(("Thing",), "alpha"),
                        beta=(("Other",), "beta"))
    for name, package in (("alpha", "alpha"), ("beta", "beta")):
        (tmp_path / name).mkdir()
        (tmp_path / name / "types.proto").write_text(f"package veldmap.{package};\n")

    # beta называет пакет alpha развёрнуто — ей это можно, alpha объявлен у неё
    # в зависимостях; для alpha же это имя своего пакета.
    (tmp_path / "beta" / "schema.yaml").write_text(
        "name: beta\n"
        "interface:\n"
        "  inputs:\n"
        "    on_x:\n"
        "      type: alpha/Thing\n"
        "dependencies:\n"
        "  alpha:\n"
        "    calls: []\n")

    schema = {"name": "alpha",
              "interface": {"outputs": {"on_ready": {"type": "module/Thing"}}},
              "dependencies": {"beta": {"calls": ["on_x"]}}}
    errors, resolved = gen.validate_module_schema(
        schema, str(tmp_path / "alpha"), PROTO_DIR, uni)
    assert errors == [], errors
    assert resolved["calls"][("beta", "on_x")] == "alpha/Thing"


def test_foreign_module_type_requires_a_declared_dependency(gen, tmp_path):
    # Тип чужого модуля виден только через объявленную зависимость: иначе связь
    # существовала бы в коде, но не в схеме.
    uni = make_universe(core=(("ResourceOpened",), "core"),
                        other=(("Thing",), "other-module"))
    schema = {"name": "svc", "interface": {"inputs": {"on_x": {"type": "other/Thing"}}}}
    module_dir = tmp_path / "svc"
    module_dir.mkdir()
    errors, _ = gen.validate_module_schema(schema, str(module_dir), PROTO_DIR, uni)
    assert any("not declared in dependencies" in e for e in errors), errors


# ── Имя сервиса ──────────────────────────────────────────────────────────────
#
# Производитель строит топик из `schema.name`, потребитель — из имени каталога,
# хост ищет конфиг по нему же. Разъехавшись, эти строки не дают ни ошибки
# компиляции, ни ошибки загрузки — событие просто не доставляется.

@pytest.mark.parametrize("name, path, ok", [
    pytest.param("data-library", "/x/veldmodules/data-library/schema.yaml", True,
                 id="модуль: имя совпадает с каталогом"),
    pytest.param("wrong",        "/x/veldmodules/data-library/schema.yaml", False,
                 id="модуль: имя разошлось с каталогом"),
    pytest.param("fs",    "/x/interface/modules/fs/fs.schema.yaml", True,
                 id="платформа: имя совпадает с файлом"),
    pytest.param("wrong", "/x/interface/modules/fs/fs.schema.yaml", False,
                 id="платформа: имя разошлось с файлом"),
])
def test_schema_name_must_match_its_location(gen, name, path, ok):
    errors = gen.validate_schema_identity({"name": name}, path)
    assert (errors == []) is ok


def test_schema_without_a_name_is_rejected(gen):
    assert len(gen.validate_schema_identity({}, "/x/a/schema.yaml")) == 1


# ── Имена типов ──────────────────────────────────────────────────────────────
#
# Схема хранит proto-имя, а стаб печатает имя, которое сгенерил prost.
# Разойдясь, они дают ошибку компиляции в сгенерированном крейте — там, где её
# труднее всего связать с причиной.

@pytest.mark.parametrize("proto_name, rust_name", [
    ("UIEvent",            "UiEvent"),
    ("ResourceOpened",     "ResourceOpened"),
    ("FsDownloadProgress", "FsDownloadProgress"),
])
def test_rust_type_name_matches_prost(gen, proto_name, rust_name):
    assert gen.rust_type_name(proto_name) == rust_name


def test_schema_type_becomes_a_rust_path(gen):
    assert gen.schema_type_to_rust_path("app/UIEvent") == "app::UiEvent"
    assert gen.schema_type_to_rust_path("core/ResourceOpened") == "core::ResourceOpened"


# ── Флаги топиков ────────────────────────────────────────────────────────────
#
# По ним шаблон решает, принимает ли стаб correlation_id и target: ошибка здесь
# меняет сигнатуру стаба, а не только документацию.

@pytest.fixture
def outputs(gen):
    schema = {"interface": {
        "inputs":  {"on_read": {"type": "fs/FsReadRequest"}},
        "outputs": {
            "on_read_result": {"type": "fs/FsReadResult", "replies_to": "on_read"},
            "on_state":       {"type": "fs/FsReadResult", "snapshot": True},
            "on_ui_event":    {"type": "fs/FsReadResult", "targeted": True},
        }}}
    entries = gen.topic_entries("svc", schema, "outputs", lambda kind, n, d: "X")
    return {e["name"]: e for e in entries}


@pytest.mark.parametrize("topic, flag, expected", [
    ("on_read_result", "correlated", True),
    ("on_state",       "correlated", False),
    ("on_ui_event",    "targeted",   True),
    ("on_state",       "targeted",   False),
    ("on_state",       "snapshot",   True),
    ("on_ui_event",    "snapshot",   False),
])
def test_topic_flags(outputs, topic, flag, expected):
    assert outputs[topic][flag] is expected


def test_only_requested_topics_are_emitted(gen):
    # Потребителю нужны лишь объявленные им вызовы, а не все входы
    # производителя.
    schema = {"interface": {"outputs": {
        "on_a": {"type": "fs/FsReadResult"},
        "on_b": {"type": "fs/FsReadResult"}}}}
    entries = gen.topic_entries("svc", schema, "outputs",
                                lambda kind, n, d: "X", only={"on_a"})
    assert [e["name"] for e in entries] == ["on_a"]


# ── Снимок состояния ─────────────────────────────────────────────────────────
#
# `snapshot: true` меняет не документацию, а поведение стаба: он помнит
# отпечаток отправленного и повтор не шлёт. Ошибка здесь молчалива вдвойне —
# событие не доставляется, и в логе об этом нет ни строки. Правила ниже
# закрывают три случая, где пометка означала бы не то, что написано, и один,
# где она безопасна ровно до первого убийства инстанса.

def snapshot_schema(kind, topic_extra=None, cancellable=False, name="svc"):
    """Схема с одним снимком заданного направления и, по просьбе, отменяемым
    входом рядом."""
    entry = {"type": "fs/FsReadResult", "snapshot": True, **(topic_extra or {})}
    iface = {"inputs": {}, "outputs": {}}
    iface[kind]["on_state"] = entry
    if cancellable:
        iface["inputs"]["on_work"] = {"type": "fs/FsReadRequest", "cancellable": True}
        iface["outputs"]["on_done"] = {"type": "fs/FsReadResult", "replies_to": "on_work"}
    return {"name": name, "interface": iface}


def test_a_plain_snapshot_output_is_accepted(gen):
    assert gen.snapshot_errors(snapshot_schema("outputs"), native=False) == []


def test_snapshot_is_rejected_in_a_platform_service(gen):
    # Стаб со снимком печатает только шаблон wasm-модуля; у платформенного
    # сервиса пометка молча не сделала бы ничего.
    errors = gen.snapshot_errors(snapshot_schema("outputs"), native=True)
    assert any("платформенного сервиса" in e for e in errors), errors


def test_snapshot_is_rejected_with_targeted(gen):
    # Отпечаток у топика один, а адресатов много: второму уехало бы
    # «не изменилось» вместо состояния, которого он ещё не видел.
    schema = snapshot_schema("outputs", {"targeted": True})
    errors = gen.snapshot_errors(schema, native=False)
    assert any("targeted" in e for e in errors), errors


@pytest.mark.parametrize("kind, extra", [
    pytest.param("outputs", {"replies_to": "on_work"}, id="ответ на запрос"),
    pytest.param("inputs",  {},                        id="запрос с ответом"),
])
def test_snapshot_is_rejected_on_a_correlated_topic(gen, kind, extra):
    # Ответ принадлежит своему запросу: совпав с прошлым ответом дословно, он
    # всё равно обязан уехать — иначе заказчик не дождётся своего.
    schema = {"name": "svc", "interface": {
        "inputs":  {"on_work": {"type": "fs/FsReadRequest",
                                **({"snapshot": True} if kind == "inputs" else {})}},
        "outputs": {"on_done": {"type": "fs/FsReadResult", "replies_to": "on_work",
                                **({"snapshot": True} if kind == "outputs" else {})}}}}
    errors = gen.snapshot_errors(schema, native=False)
    assert any("replies_to" in e for e in errors), errors


def test_snapshot_input_is_rejected_in_a_cancellable_service(gen):
    # Убитый инстанс поднимается с чистой памятью и теряет присланное, а
    # отправитель помнит отправленное и повтора не сделает.
    errors = gen.snapshot_errors(snapshot_schema("inputs", cancellable=True), native=False)
    assert any("отменяемого" in e for e in errors), errors


def test_snapshot_output_of_a_cancellable_service_is_fine(gen):
    # Терять нечего тому, кто рассылает: свежий инстанс начинает с «ещё не
    # слали» и первую рассылку делает непременно.
    assert gen.snapshot_errors(snapshot_schema("outputs", cancellable=True),
                               native=False) == []


@pytest.mark.parametrize("cancellable, ok", [
    pytest.param(False, True,  id="подписчик не убиваем"),
    pytest.param(True,  False, id="подписчик отменяем"),
])
def test_subscribing_to_a_snapshot_requires_surviving(gen, universe, tmp_path,
                                                      cancellable, ok):
    # Вторая половина того же правила, и проверить её можно только
    # перекрёстно: снимок объявлен у производителя, а теряет состояние
    # подписчик.
    producer = tmp_path / "producer"
    producer.mkdir()
    (producer / "schema.yaml").write_text(
        "name: producer\n"
        "interface:\n"
        "  outputs:\n"
        "    on_state:\n"
        "      type: fs/FsReadResult\n"
        "      snapshot: true\n")

    consumer = tmp_path / "consumer"
    consumer.mkdir()
    schema = snapshot_schema("outputs", cancellable=cancellable, name="consumer")
    schema["interface"]["outputs"].pop("on_state")
    schema["dependencies"] = {"producer": {"subs": ["on_state"]}}

    errors, _ = gen.validate_module_schema(schema, str(consumer), PROTO_DIR, universe)
    assert (errors == []) is ok, errors
    if not ok:
        assert any("рассылает снимок" in e for e in errors), errors


def test_project_schemas_declare_their_snapshots(gen, real_schemas):
    # Регрессия на живых схемах: рассылка состояния целиком — не команда, и
    # ограничитель у неё должен быть от кодогена, а не от руки.
    declared = {f"{name}/{topic}"
                for name, _path, schema, _kind in real_schemas
                for direction in ("inputs", "outputs")
                for topic, entry in ((schema.get("interface") or {}).get(direction) or {}).items()
                if (entry or {}).get("snapshot")}
    assert declared == {
        "data-library/on_state",
        "globe/on_outlines",
        "globe/on_overlay",
        "globe/on_overlay_progress",
        "image-view/on_view_state",
        "ui-service/on_set_view",
    }
