#!/usr/bin/env python3
"""Прогон сценариев интерфейса из uitests/.

Интерфейс юнит-тестами не проверяется — «как это выглядит» видно только
запуском (см. CLAUDE.md). Сценарий делает запуск повторяемым, а этот скрипт —
пакетным: гоняет по сценарию за раз и сводит исходы в один код возврата.

Каждый сценарий — отдельный запуск приложения: состояние окна, кэш тайлов и
разобранные снимки живут между шагами, и делить их между сценариями значило бы
ставить исход одного в зависимость от того, что успел сделать другой.

Раскладка окна (runtime/state/data-browser.json) на время прогона убирается и
возвращается в конце: она переживает запуск, и сценарий, начинающийся «с той
вкладки, что была», не воспроизводим. Без неё окно открывается тем, что задано
в runtime/config.

    python3 buildgen/run-uitests.py            # все сценарии
    python3 buildgen/run-uitests.py tabs menus # названные

Снимки кадров ложатся в runtime/logs/, как и при обычном прогоне. Хост
перезаписывает host.log на каждом старте, поэтому после каждого сценария его
копия остаётся рядом под именем сценария: иначе от провалившегося первым не
осталось бы ничего — его лог затёрли бы следующие.
"""
import os
import subprocess
import sys
import time

BUILDGEN_DIR = os.path.dirname(os.path.abspath(__file__))
PROJECT_ROOT = os.path.normpath(os.path.join(BUILDGEN_DIR, ".."))
SCENARIOS_DIR = os.path.join(PROJECT_ROOT, "uitests")
WINDOW_STATE = os.path.join(PROJECT_ROOT, "runtime", "state", "data-browser.json")
HOST_LOG = os.path.join(PROJECT_ROOT, "runtime", "logs", "host.log")

# Предел на один сценарий. Сам сценарий кончается шагом `exit`; сюда прогон
# доходит, только если приложение зависло или не дошло до конца — и тогда это
# провал, а не повод ждать дальше.
LIMIT_SECONDS = 180


def scenarios(names: list[str]) -> list[str]:
    """Пути сценариев: названные или все, в алфавитном порядке."""
    if names:
        return [os.path.join(SCENARIOS_DIR, f"{name}.txt") for name in names]
    if not os.path.isdir(SCENARIOS_DIR):
        return []
    return [os.path.join(SCENARIOS_DIR, name)
            for name in sorted(os.listdir(SCENARIOS_DIR)) if name.endswith(".txt")]


def play(path: str, name: str) -> tuple[bool, float]:
    """Один сценарий: True — сошёлся."""
    env = os.environ.copy()
    env["VELDMAP_SCRIPT"] = path
    started = time.monotonic()
    try:
        done = subprocess.run(
            [sys.executable, os.path.join(BUILDGEN_DIR, "run-native.py")],
            cwd=PROJECT_ROOT, env=env, timeout=LIMIT_SECONDS,
            stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT,
        )
        passed = done.returncode == 0
    except subprocess.TimeoutExpired:
        passed = False
    keep_log(name)
    return passed, time.monotonic() - started


def keep_log(name: str) -> None:
    """Копия лога сценария рядом с ним: следующий запуск host.log затрёт."""
    if not os.path.exists(HOST_LOG):
        return
    with open(HOST_LOG, "rb") as source:
        text = source.read()
    with open(os.path.join(os.path.dirname(HOST_LOG), f"{name}.log"), "wb") as kept:
        kept.write(text)


def main() -> int:
    found = scenarios(sys.argv[1:])
    if not found:
        print(f"Сценариев нет: {SCENARIOS_DIR}")
        return 1

    # Раскладку окна убираем на время прогона и возвращаем в конце — она
    # хозяйская, а не наша.
    stashed = None
    if os.path.exists(WINDOW_STATE):
        with open(WINDOW_STATE, "rb") as f:
            stashed = f.read()

    failed = []
    try:
        for path in found:
            name = os.path.splitext(os.path.basename(path))[0]
            if not os.path.exists(path):
                print(f"  {name}: нет такого сценария")
                failed.append(name)
                continue
            if os.path.exists(WINDOW_STATE):
                os.remove(WINDOW_STATE)
            print(f"  {name}: ", end="", flush=True)
            passed, spent = play(path, name)
            print(f"{'сошёлся' if passed else 'НЕ СОШЁЛСЯ'} — {spent:.1f}s")
            if not passed:
                failed.append(name)
    finally:
        if stashed is not None:
            os.makedirs(os.path.dirname(WINDOW_STATE), exist_ok=True)
            with open(WINDOW_STATE, "wb") as f:
                f.write(stashed)

    print()
    if failed:
        print(f"Не сошлись: {', '.join(failed)} (из {len(found)}). "
              f"Причина — в runtime/logs/<имя>.log.")
        return 1
    print(f"Сошлись все {len(found)}.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
