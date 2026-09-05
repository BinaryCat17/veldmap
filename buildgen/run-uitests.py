#!/usr/bin/env python3
"""Прогон сценариев интерфейса из uitests/.

Интерфейс юнит-тестами не проверяется — «как это выглядит» видно только
запуском (см. docs/operations/ui-tests.md). Сценарий делает запуск повторяемым, а этот скрипт —
пакетным: гоняет по сценарию за раз и сводит исходы в один код возврата.

Каждый сценарий — отдельный запуск приложения: состояние окна, кэш тайлов и
разобранные снимки живут между шагами, и делить их между сценариями значило бы
ставить исход одного в зависимость от того, что успел сделать другой.

Два состояния переживают запуск, и оба на время прогона убираются, а в конце
возвращаются: раскладка окна (runtime/state/data-browser.json) — с ней
сценарий, начинающийся «с той вкладки, что была», не воспроизводим, — и кэш
тайлов (runtime/data/tiles), с которым снимок, показанный прошлым прогоном,
приезжает с диска, и ни декодер, ни провод в сценарии не участвуют. Сам
хозяйский кэш прогону не нужен: он откладывается в сторону целиком, а кэш,
набранный сценариями, стирается перед каждым следующим.

    python3 buildgen/run-uitests.py            # все сценарии
    python3 buildgen/run-uitests.py tabs menus # названные

Снимки кадров ложатся в runtime/logs/, как и при обычном прогоне. Хост
перезаписывает host.log на каждом старте, поэтому после каждого сценария его
копия остаётся рядом под именем сценария: иначе от провалившегося первым не
осталось бы ничего — его лог затёрли бы следующие.
"""
import os
import re
import shutil
import signal
import subprocess
import sys
import time

BUILDGEN_DIR = os.path.dirname(os.path.abspath(__file__))
PROJECT_ROOT = os.path.normpath(os.path.join(BUILDGEN_DIR, ".."))
SCENARIOS_DIR = os.path.join(PROJECT_ROOT, "uitests")
WINDOW_STATE = os.path.join(PROJECT_ROOT, "runtime", "state", "data-browser.json")
HOST_LOG = os.path.join(PROJECT_ROOT, "runtime", "logs", "host.log")
TRACE_LOG = os.path.join(PROJECT_ROOT, "runtime", "logs", "trace.log")
# Стандартный поток ошибок хоста: паника с `panic = "abort"` уносит процесс
# молча, и единственное её слово — здесь.
HOST_STDERR = os.path.join(PROJECT_ROOT, "runtime", "logs", "host.stderr")
# Кэш тайлов (`tile-cache/src/layout.rs::ROOT`) и куда он откладывается на
# время прогона.
TILE_CACHE = os.path.join(PROJECT_ROOT, "runtime", "data", "tiles")
TILE_CACHE_ASIDE = TILE_CACHE + ".aside"

# Предел на один сценарий сверх его собственных ожиданий. Сам сценарий
# кончается шагом `exit`; сюда прогон доходит, только если приложение зависло
# или не дошло до конца — и тогда это провал, а не повод ждать дальше.
# Сценарий, объявивший `timeout` длиннее (закачка файла — минуты), получает
# столько же сверх него: иначе прогон убивал бы то, чего сценарий честно ждёт.
LIMIT_SECONDS = 180


def steps(path: str, verb: str) -> list[int]:
    """Числа при шагах сценария с данным вербом — в порядке файла.

    Комментарий режется тем же правилом, что у хоста (`capture.rs::strip_comment`):
    решётка, начинающая слово, — начало заметки; иначе закомментированный шаг
    считался бы обещанием.
    """
    found = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            fields = line.split()
            noted = next((at for at, word in enumerate(fields) if word.startswith("#")), len(fields))
            fields = fields[:noted]
            if len(fields) >= 3 and fields[1] == verb and fields[2].isdigit():
                found.append(int(fields[2]))
    return found


def limit_for(path: str) -> float:
    """Предел прогона: общий запас плюс самое долгое ожидание сценария."""
    return LIMIT_SECONDS + max(steps(path, "timeout"), default=0) / 1000


def delivered_limit(path: str) -> int | None:
    """За какую долю доставленного по сети ручается сценарий (шаг
    `delivered`), процентов от длины ресурса; None — ни за какую."""
    promised = steps(path, "delivered")
    return min(promised) if promised else None


def scenarios(names: list[str]) -> list[str]:
    """Пути сценариев: названные или все, в алфавитном порядке."""
    if names:
        return [os.path.join(SCENARIOS_DIR, f"{name}.txt") for name in names]
    if not os.path.isdir(SCENARIOS_DIR):
        return []
    return [os.path.join(SCENARIOS_DIR, name)
            for name in sorted(os.listdir(SCENARIOS_DIR)) if name.endswith(".txt")]


def stop(running: subprocess.Popen) -> None:
    """Снять прогон целиком — всю группу процессов, а не одного посредника.
    Сперва TERM — обработчика у хоста нет, и он просто умирает, а лог цел,
    потому что пишется построчно; не ушёл (посредник, ждущий потомка) —
    KILL."""
    os.killpg(running.pid, signal.SIGTERM)
    try:
        running.wait(timeout=10)
    except subprocess.TimeoutExpired:
        os.killpg(running.pid, signal.SIGKILL)
        running.wait()


def play(path: str, name: str) -> tuple[str, float]:
    """Один сценарий: чем он кончился."""
    env = os.environ.copy()
    env["VELDMAP_SCRIPT"] = path
    # Логи прошлого сценария убираем сами: хост перезаписывает их на старте, а
    # не дойдя до этого места (не разобранный конфиг, не поднявшееся окно), он
    # оставил бы на диске чужие — и отказ из них приписался бы этому прогону.
    for stale in (HOST_LOG, TRACE_LOG, HOST_STDERR):
        if os.path.exists(stale):
            os.remove(stale)
    started = time.monotonic()
    # Своей группой процессов: хост — внук (его поднимает run-native.py), и
    # убитый по пределу посредник сам по себе окно не уносит. stderr — в файл:
    # паника хоста с `panic = "abort"` кончает процесс, не дойдя до лога, и
    # без этого файла сценарий «не сошёлся» без причины.
    with open(HOST_STDERR, "wb") as stderr:
        running = subprocess.Popen(
            [sys.executable, os.path.join(BUILDGEN_DIR, "run-native.py")],
            cwd=PROJECT_ROOT, env=env, start_new_session=True,
            stdout=subprocess.DEVNULL, stderr=stderr,
        )
    promised = delivered_limit(path)
    try:
        result = outcome(running.wait(timeout=limit_for(path)), host_log(), trace_log(), promised, host_stderr())
    except subprocess.TimeoutExpired:
        stop(running)
        # Отличается от «не сошёлся» намеренно: сценарий, дошедший до своего
        # конца, кончается сам, а упёршийся в предел — это зависшее
        # приложение, и искать его причину надо не в сценарии.
        #
        # Лог смотрится и здесь, и нужнее всего он как раз здесь: шаги
        # отыгрываются по кадрам, поэтому вставший кадровый цикл держит и часы
        # сценария — свой предел ожидания не срабатывает, и прогон доезжает
        # досюда. Ответ при этом в логе уже лежит.
        result = " + ".join(["ЗАВИС"] + unseen(host_log(), stderr=host_stderr()))
    keep_log(name, promised is not None)
    return result, time.monotonic() - started


# Слова, которыми хост пишет о том, чего сценарий не видит. Здесь, а не по
# месту: печатает их Rust, ищет их прогон, и сойтись они обязаны механически —
# см. buildgen/tests/test_uitests_outcomes.py.
GPU_NEEDLE = "wgpu: "
TRAP_NEEDLE = "поймал трап"
# Слово паники Rust в stderr: печатает его стандартная библиотека, а не мы.
PANIC_NEEDLE = "panicked at"
# Строка сети о доставленном (`network::perf` в trace.log): формат, как его
# пишет range.rs, и разбор той же строки.
DELIVERED_FORMAT = "доставлено {:.1} из {:.1} МиБ ({}%)"
DELIVERED_LINE = re.compile(r"доставлено [\d.]+ из [\d.]+ МиБ \((\d+)%\)")


def read_log(path: str) -> str:
    if not os.path.exists(path):
        return ""
    with open(path, encoding="utf-8", errors="replace") as log:
        return log.read()


def host_log() -> str:
    """Лог последнего запуска целиком.

    Пусто — лога нет: хост не дошёл до его открытия. Отдельным исходом это не
    называется, потому что не бывает тихим — не поднявшийся хост возвращает
    ненулевой код, и прогон объявит отказ и без лога. Чужого лога здесь не
    бывает: прошлый убран перед запуском (см. `play`).
    """
    return read_log(HOST_LOG)


def trace_log() -> str:
    """Полный поток последнего запуска: сюда сеть пишет, сколько привезла."""
    return read_log(TRACE_LOG)


def host_stderr() -> str:
    """Стандартный поток ошибок последнего запуска — там лежит паника."""
    return read_log(HOST_STDERR)


def outcome(returncode: int, log: str, trace: str = "", promised: int | None = None, stderr: str = "") -> str:
    """Чем кончился прогон: код возврата плюс то, чего сценарий не видит.

    Лог смотрится при любом коде возврата, а найденное в нём дополняет вердикт,
    а не подменяет: провалившийся сценарий с отказом под ним — это два факта, и
    нужны оба. Почему так и почему порядок отказов постоянный — в
    docs/operations/ui-tests.md.
    """
    found = unseen(log, trace, promised, stderr)
    if returncode == 0:
        return " + ".join(found) if found else "сошёлся"
    return " + ".join(["НЕ СОШЁЛСЯ"] + found)


def unseen(log: str, trace: str = "", promised: int | None = None, stderr: str = "") -> list[str]:
    """Отказы, которых сценарий не заметил, — в порядке, названном у `outcome`."""
    found = [name for name, seen in (("ОТКАЗ ВИДЕОКАРТЫ", gpu_refused(log)),
                                     ("ТРАП МОДУЛЯ", module_trapped(log))) if seen]
    found.extend(panicked(stderr))
    if promised is not None:
        found.extend(broken_promise(trace, promised))
    return found


def panicked(stderr: str) -> list[str]:
    """Паника хоста — словом и первой строкой причины.

    С `panic = "abort"` процесс кончается на панике любого потока, лог до этого
    не доходит, и прогон видел бы лишь «не сошёлся»; причина при этом лежит в
    stderr строкой за «panicked at». Она и печатается: без неё вердикт называл
    бы падение, но не говорил, отчего.
    """
    lines = stderr.splitlines()
    for at, line in enumerate(lines):
        if PANIC_NEEDLE in line:
            reason = lines[at + 1].strip() if at + 1 < len(lines) else ""
            return [f"ПАНИКА ХОСТА: {reason}"[:160] if reason else "ПАНИКА ХОСТА"]
    return []


def broken_promise(trace: str, promised: int) -> list[str]:
    """Чем не сдержано ручательство за провод: долей больше обещанной либо
    тем, что провода не было вовсе.

    Сценарий за провод ручается шагом `delivered`, а байты считает сеть и
    пишет их в trace.log нарастающим итогом по каждому ресурсу — так что
    последняя строка ресурса и есть его итог, а наибольшая по всем строкам —
    худший ресурс прогона. Ни одной строки — значит, ни один ресурс по сети не
    читался: снимок открыт с диска либо приехал из кэша, и ручательство
    сошлось бы ровно там, где проверять нечего.
    """
    shares = [int(share) for share in DELIVERED_LINE.findall(trace)]
    if not shares:
        return ["РУЧАТЕЛЬСТВО БЕЗ ПРОВОДА"]
    worst = max(shares)
    return [f"ДОСТАВЛЕНО {worst}% ПРИ ОБЕЩАННЫХ {promised}%"] if worst > promised else []


def gpu_refused(log: str) -> bool:
    """Отказала ли видеокарта чему-нибудь за этот запуск.

    Ищется в логе, а не спрашивается у сценария: шейдер, не собравшийся, и
    раскладка вершин, не сошедшаяся с ним, не роняют приложение — окно живёт,
    разметка отвечает, а рисуется пустое место. Ни один шаг сценария такого не
    замечает: у шара и канвы просмотра имён нет.

    По приставке, а не по словам самого отказа: их три рода — проверка,
    нехватка памяти и внутренняя ошибка, — и печатает их всех один обработчик
    (`veldcore/platform/host/core/src/setup.rs`). Ловля по «Validation Error»
    пропустила бы нехватку памяти, которая называет себя иначе.
    """
    return GPU_NEEDLE in log


def module_trapped(log: str) -> bool:
    """Падал ли за этот запуск какой-нибудь модуль.

    Тот же род незаметности, что у видеокарты, и потому та же ловля. Трап
    отравляет Store безвозвратно, поэтому хост собирает инстанс заново и
    прогоняет init — состояние модуля теряется целиком, как при отключении
    электричества. Приложение при этом живёт, и упавший модуль отвечает
    дальше — просто с чистого листа.

    Обмены, которые модуль успел начать, хост договаривает за него терминальным
    ответом — всякий учтённый, не только отменяемый
    (`plugins.rs::answer_for_lost`); непочатая очередь падение переживает, и
    на неё отвечает поднятый инстанс.

    Ищется падение, а не всякий подъём инстанса: снятого посреди обработчика
    хост поднимает тем же ходом, и говорит он об этом `info` — это норма, а не
    отказ (`plugins.rs::revive`).
    """
    return TRAP_NEEDLE in log


def keep_log(name: str, with_trace: bool) -> None:
    """Копия лога сценария рядом с ним: следующий запуск host.log затрёт.

    Полный поток остаётся только у сценария с ручательством за провод: вердикт
    вынесен по его строкам, и числа за вердиктом должны быть под рукой, а у
    прочих сценариев trace.log — мегабайты ни о чём.
    """
    for source, kept in ((HOST_LOG, f"{name}.log"), (TRACE_LOG, f"{name}.trace.log"),
                         (HOST_STDERR, f"{name}.stderr")):
        if source == TRACE_LOG and not with_trace:
            continue
        if not os.path.exists(source) or (source == HOST_STDERR and os.path.getsize(source) == 0):
            continue
        with open(source, "rb") as original:
            text = original.read()
        with open(os.path.join(os.path.dirname(HOST_LOG), kept), "wb") as copy:
            copy.write(text)


def main() -> int:
    found = scenarios(sys.argv[1:])
    if not found:
        print(f"Сценариев нет: {SCENARIOS_DIR}")
        return 1

    # Раскладку окна и кэш тайлов убираем на время прогона и возвращаем в
    # конце — они хозяйские, а не наши. Кэш не копируется, а откладывается
    # переименованием: в нём сотни мегабайт, и они остаются где лежали.
    stashed = None
    if os.path.exists(WINDOW_STATE):
        with open(WINDOW_STATE, "rb") as f:
            stashed = f.read()
    if os.path.isdir(TILE_CACHE) and not os.path.exists(TILE_CACHE_ASIDE):
        os.rename(TILE_CACHE, TILE_CACHE_ASIDE)

    failed = []
    try:
        for path in found:
            name = os.path.splitext(os.path.basename(path))[0]
            if not os.path.exists(path):
                print(f"  {name}: нет такого сценария")
                failed.append(f"{name} (нет файла)")
                continue
            if os.path.exists(WINDOW_STATE):
                os.remove(WINDOW_STATE)
            shutil.rmtree(TILE_CACHE, ignore_errors=True)
            print(f"  {name}: ", end="", flush=True)
            result, spent = play(path, name)
            print(f"{result} — {spent:.1f}s")
            if result != "сошёлся":
                failed.append(f"{name} ({result.lower()})")
    finally:
        if stashed is not None:
            os.makedirs(os.path.dirname(WINDOW_STATE), exist_ok=True)
            with open(WINDOW_STATE, "wb") as f:
                f.write(stashed)
        if os.path.exists(TILE_CACHE_ASIDE):
            shutil.rmtree(TILE_CACHE, ignore_errors=True)
            os.rename(TILE_CACHE_ASIDE, TILE_CACHE)

    print()
    if failed:
        print(f"Не сошлись: {', '.join(failed)} (из {len(found)}). "
              f"Причина — в runtime/logs/<имя>.log.")
        return 1
    print(f"Сошлись все {len(found)}.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
