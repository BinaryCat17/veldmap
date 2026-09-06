"""Исходы прогона, которые видны только в логе.

Сценарий проверяет то, что умеет назвать: нажимаемые коробки, поля ввода и
надписи. Отказ видеокарты и упавший модуль он назвать не может — у шара и
канвы имён нет, а состояние модуля именами вообще не выражается, — поэтому
`run-uitests.py` ищет и то, и другое в `host.log` по словам, которыми хост об
этом пишет.

Слова эти живут в двух местах: печатает их Rust, ищет их прогон. Разойдясь (а
разойтись они могут от одной переформулировки), они не ломают ни сборку, ни сам
прогон — прогон просто перестаёт видеть отказ. Здесь эти два места и сводятся:
игла берётся у прогона, а ищется в исходнике хоста, так что переписать
согласованно можно, а порознь нельзя.

Отдельно проверяется решение об исходе целиком: что найденное в логе
дополняет код возврата, а не подменяет его.
"""
import importlib.util
import os

from conftest import BUILDGEN_DIR, PROJECT_ROOT

HOST_SRC = os.path.join(PROJECT_ROOT, "veldcore", "platform", "host", "core", "src")
NETWORK_SRC = os.path.join(PROJECT_ROOT, "veldcore", "platform", "host", "modules", "network", "src")

# Отрицательный пример: снятый посреди обработчика поднимается тем же ходом, что
# упавший, и спутать их значило бы валить всякий прогон, в котором что-нибудь
# отменили. Прогон эту строку не ищет, поэтому и живёт она здесь.
KILL_NOTE = "снят посреди обработчика"


def load_runner():
    """Сам прогон: имя файла с дефисом, обычным import его не взять."""
    path = os.path.join(BUILDGEN_DIR, "run-uitests.py")
    spec = importlib.util.spec_from_file_location("run_uitests", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def prints(source: str, needle: str) -> bool:
    """Печатает ли хост это сам — в строке, а не в комментарии."""
    with open(os.path.join(HOST_SRC, source), encoding="utf-8") as f:
        return any(needle in line and "log::" in line for line in f)


def test_the_runner_reads_the_words_the_host_prints():
    """Обе иглы прогона встречаются там, где хост об этом пишет.

    Про видеокарту пишут два места, и оба обязательны. `setup.rs` — обработчик
    неперехваченного: рисование, шейдеры, раскладки. `memory.rs` — отказ
    выделения, и сюда он попадает потому, что ловится областью ошибок на месте
    и до обработчика не доходит вовсе. Проверять одно из двух значит ослепнуть
    ровно наполовину, не заметив этого.
    """
    runner = load_runner()

    assert prints("setup.rs", runner.GPU_NEEDLE), \
        f"'{runner.GPU_NEEDLE}' больше не печатается в setup.rs — ловля ослепла"
    assert prints("memory.rs", runner.GPU_NEEDLE), \
        f"'{runner.GPU_NEEDLE}' больше не печатается в memory.rs — отказ выделения " \
        "перестал быть виден прогону"
    assert prints("plugins.rs", runner.TRAP_NEEDLE), \
        f"'{runner.TRAP_NEEDLE}' больше не печатается в plugins.rs — ловля ослепла"


def test_a_killed_module_is_not_a_trap():
    """Снятый посреди обработчика поднимается тем же ходом — и это норма.

    Ловля подъёма инстанса вместо падения валила бы всякий прогон, в котором
    что-нибудь отменили: убийство `produce` при смене масштаба — обычная работа
    шара и канвы. Поэтому проверяется и то, что хост эти два случая по-прежнему
    различает, и то, что предикат не путает их между собой.
    """
    runner = load_runner()

    assert prints("plugins.rs", KILL_NOTE), \
        "хост больше не отличает снятие от падения — предикат стал ненадёжен"
    assert not runner.module_trapped(f"... {KILL_NOTE} ...\n")


def test_the_unseen_refusal_outranks_the_return_code():
    """Отказ, которого сценарий не видит, называется и у провалившегося прогона.

    Это и есть вся суть ловли: падение валит сценарий ничуть не реже, чем
    обходится незамеченным (почему — в docs/operations/ui-tests.md).
    """
    runner = load_runner()
    trap = f"... {runner.TRAP_NEEDLE} ...\n"

    assert runner.outcome(0, trap) == "ТРАП МОДУЛЯ"
    assert runner.outcome(1, trap) == "НЕ СОШЁЛСЯ + ТРАП МОДУЛЯ"


def test_every_refusal_is_named_and_the_verdict_survives():
    """Найденное дополняет код возврата, а не заменяет его.

    Провалившийся сценарий с отказом под ним — это два факта, и нужны оба:
    первый говорит, что проверка не сошлась, второй — что причину не надо
    искать в сценарии. Назови мы только второй, и «а сошлось ли вообще»
    осталось бы без ответа.
    """
    runner = load_runner()
    both = f"... {runner.GPU_NEEDLE} ...\n... {runner.TRAP_NEEDLE} ...\n"

    assert runner.outcome(0, both) == "ОТКАЗ ВИДЕОКАРТЫ + ТРАП МОДУЛЯ"
    assert runner.outcome(1, both) == "НЕ СОШЁЛСЯ + ОТКАЗ ВИДЕОКАРТЫ + ТРАП МОДУЛЯ"


def test_a_host_panic_is_named_with_its_reason():
    """Паника хоста видна вердикту вместе с причиной: с `panic = "abort"` она
    не доходит до лога, и без stderr сценарий кончался бы «не сошёлся» ни с
    чего.
    """
    runner = load_runner()
    stderr = (
        "thread '<unnamed>' (1) panicked at /rustc/x/library/core/src/ops/function.rs:250:5:\n"
        "there is no reactor running, must be called from the context of a Tokio 1.x runtime\n"
        "note: run with `RUST_BACKTRACE=1`\n"
    )

    verdict = runner.outcome(134, "", stderr=stderr)
    assert verdict.startswith("НЕ СОШЁЛСЯ (SIGABRT) + ПАНИКА ХОСТА: there is no reactor running"), verdict
    assert runner.outcome(0, "", stderr="") == "сошёлся"
    assert runner.panicked("thread 'x' panicked at a.rs:1:1:") == ["ПАНИКА ХОСТА"], "без строки причины — одно слово"

    # Паника гостя пишется в тот же stderr (модули наследуют его через WASI),
    # но хост её переживает трапом: паникой хоста она не называется.
    trapped = f"... {runner.TRAP_NEEDLE} ...\n"
    assert runner.outcome(0, trapped, stderr=stderr) == "ТРАП МОДУЛЯ"
    assert runner.outcome(1, "", stderr=stderr) == "НЕ СОШЁЛСЯ", "умер не от паники — не паника"


def test_a_quiet_log_leaves_the_return_code_alone():
    """Без отказов исход решает только код возврата."""
    runner = load_runner()

    assert runner.outcome(0, "") == "сошёлся"
    assert runner.outcome(1, "") == "НЕ СОШЁЛСЯ"


def test_a_signal_is_named_in_the_verdict():
    """Смерть хоста от сигнала названа по имени: код 128+N — так её отдаёт
    лаунчер (`run-native.py::exit_code`), и только имя различает панику,
    падение и убийство снаружи, когда stderr пуст.
    """
    runner = load_runner()

    assert runner.outcome(139, "") == "НЕ СОШЁЛСЯ (SIGSEGV)"
    assert runner.outcome(137, "") == "НЕ СОШЁЛСЯ (SIGKILL)"
    assert runner.outcome(250, "") == "НЕ СОШЁЛСЯ", "код без сигнала за ним — просто код"
    assert runner.outcome(-9, "") == "НЕ СОШЁЛСЯ (посредник убит SIGKILL)", "убит сам лаунчер — не хост"


def test_the_delivered_line_is_read_as_the_network_writes_it():
    """Строка о доставленном разбирается тем же форматом, каким её пишет сеть.

    Формат лежит в range.rs строкой Rust, разбор — регулярным выражением
    прогона; разойдись они, прогон перестал бы видеть провод и молча считал бы
    всякое ручательство сдержанным. Сводятся они здесь: формат берётся у
    прогона и ищется в исходнике сети, а выражение проверяется на строке,
    собранной по этому формату.
    """
    runner = load_runner()
    with open(os.path.join(NETWORK_SRC, "range.rs"), encoding="utf-8") as f:
        assert runner.DELIVERED_FORMAT in f.read(), \
            f"'{runner.DELIVERED_FORMAT}' больше не печатается в range.rs — ручательство за провод ослепло"
    line = "ресурс 7: " + runner.DELIVERED_FORMAT.replace("{:.1}", "61.5", 1).replace("{:.1}", "128.8", 1) \
        .replace("{}", "47") + ", запросов 121 по 512 КиБ"
    assert runner.DELIVERED_LINE.findall(line) == ["47"]


def test_the_delivery_promise_is_checked_against_the_worst_resource():
    """Ручательство сценария сверяется с наибольшей долей по всем строкам:
    доля у ресурса нарастает, так что его последняя строка и есть итог, а
    худший ресурс решает исход. Без ручательства провод не смотрится, а
    ручательство без единой строки сети — не сдержано: провода не было."""
    runner = load_runner()
    trace = ("… ресурс 7: доставлено 4.0 из 128.8 МиБ (3%), запросов 8 по 512 КиБ …\n"
             "… ресурс 7: доставлено 61.5 из 128.8 МиБ (47%), запросов 121 по 512 КиБ …\n"
             "… ресурс 9: доставлено 0.3 из 0.3 МиБ (100%), запросов 1 по 270 КиБ …\n")
    assert runner.broken_promise(trace, 100) == []
    assert runner.broken_promise(trace, 75) == ["ДОСТАВЛЕНО 100% ПРИ ОБЕЩАННЫХ 75%"]
    assert runner.outcome(0, "", trace, None) == "сошёлся", "без ручательства провод не смотрится"
    assert runner.outcome(0, "", trace, 50) == "ДОСТАВЛЕНО 100% ПРИ ОБЕЩАННЫХ 50%"
    assert runner.outcome(1, "", trace, 50) == "НЕ СОШЁЛСЯ + ДОСТАВЛЕНО 100% ПРИ ОБЕЩАННЫХ 50%"
    assert runner.outcome(0, "", "", 50) == "РУЧАТЕЛЬСТВО БЕЗ ПРОВОДА"


def test_the_promise_is_read_from_the_scenario(tmp_path):
    """Доля берётся у шага `delivered`, а при нескольких — наименьшая."""
    runner = load_runner()
    quiet = tmp_path / "quiet.txt"
    quiet.write_text("100 wait tab_menu\n200 exit\n", encoding="utf-8")
    promising = tmp_path / "promising.txt"
    promising.write_text("100 delivered 80\n200 delivered 60 # строже\n#300 delivered 10\n"
                         "# 400 delivered 5\n500 exit\n", encoding="utf-8")
    assert runner.delivered_limit(str(quiet)) is None
    assert runner.delivered_limit(str(promising)) == 60, "закомментированный шаг — не обещание"


def test_the_run_limit_grows_with_the_scenario_wait(tmp_path):
    """Сценарий, ждущий закачку минутами, не должен быть убит общим пределом:
    предел прогона — запас сверх самого долгого объявленного ожидания."""
    runner = load_runner()
    short = tmp_path / "short.txt"
    short.write_text("100 wait tab_menu\n200 exit\n", encoding="utf-8")
    long = tmp_path / "long.txt"
    long.write_text("100 timeout 10000\n200 timeout 900000\n300 gone text:x\n400 timeout 30000\n",
                    encoding="utf-8")
    assert runner.limit_for(str(short)) == runner.LIMIT_SECONDS
    assert runner.limit_for(str(long)) == runner.LIMIT_SECONDS + 900
