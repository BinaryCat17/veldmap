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


# ── Отменяемость и таблица потока ────────────────────────────────────────────
#
# `cancellable` объявляет исполнитель, а хост по нему открывает учёт. Учёт,
# который нечем закрыть, — незакрываемая задача, поэтому пара обязательна.

def test_cancellable_request_yields_fully_qualified_topics(gen):
    # Таблица уезжает в хост как есть: в ней топики вида `<сервис>/<имя>`,
    # потому что диспетчер сравнивает именно их.
    schema = schema_with({"on_done": reply("on_work")}, cancellable=True,
                         name="image-loader")
    entries, errors = gen.flow_entries("image-loader", schema)
    assert errors == []
    assert entries == [{"request":  "image-loader/on_work",
                        "terminal": "image-loader/on_done"}]


def test_non_cancellable_request_is_not_accounted(gen):
    # Учёт заводится только на объявленно долгую работу: у остальных операций
    # нечего убивать, и запись о них была бы мусором.
    entries, errors = gen.flow_entries("fs", schema_with({"on_done": reply("on_work")}))
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
            "on_state":       {"type": "fs/FsReadResult"},
            "on_ui_event":    {"type": "fs/FsReadResult", "targeted": True},
        }}}
    entries = gen.topic_entries("svc", schema, "outputs", lambda kind, n, d: "X")
    return {e["name"]: e for e in entries}


@pytest.mark.parametrize("topic, flag, expected", [
    ("on_read_result", "correlated", True),
    ("on_state",       "correlated", False),
    ("on_ui_event",    "targeted",   True),
    ("on_state",       "targeted",   False),
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
