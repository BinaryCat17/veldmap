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
    """Обе иглы прогона встречаются там, где хост об этом пишет."""
    runner = load_runner()

    assert prints("setup.rs", runner.GPU_NEEDLE), \
        f"'{runner.GPU_NEEDLE}' больше не печатается в setup.rs — ловля ослепла"
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
    обходится незамеченным (почему — в CLAUDE.md).
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


def test_a_quiet_log_leaves_the_return_code_alone():
    """Без отказов исход решает только код возврата."""
    runner = load_runner()

    assert runner.outcome(0, "") == "сошёлся"
    assert runner.outcome(1, "") == "НЕ СОШЁЛСЯ"
