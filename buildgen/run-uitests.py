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


def play(path: str, name: str) -> tuple[str, float]:
    """Один сценарий: чем он кончился."""
    env = os.environ.copy()
    env["VELDMAP_SCRIPT"] = path
    started = time.monotonic()
    try:
        done = subprocess.run(
            [sys.executable, os.path.join(BUILDGEN_DIR, "run-native.py")],
            cwd=PROJECT_ROOT, env=env, timeout=LIMIT_SECONDS,
            stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT,
        )
        outcome = "сошёлся" if done.returncode == 0 else "НЕ СОШЁЛСЯ"
        if outcome == "сошёлся" and gpu_refused():
            # Сценарий дошёл до конца и проверки сошлись — а видеокарта при
            # этом отказалась от того, чем рисуют. Разметку это не трогает:
            # шар и канва просмотра именами не адресуются, и обход их не
            # видит, — так что дошедший до конца сценарий скажет «сошёлся»
            # над пустым местом. Отказ здесь и есть единственный след.
            outcome = "ОТКАЗ ВИДЕОКАРТЫ"
    except subprocess.TimeoutExpired:
        # Отличается от «не сошёлся» намеренно: сценарий, дошедший до своего
        # конца, кончается сам, а упёршийся в предел — это зависшее
        # приложение, и искать его причину надо не в сценарии.
        outcome = "ЗАВИС"
    keep_log(name)
    return outcome, time.monotonic() - started


def gpu_refused() -> bool:
    """Отказала ли видеокарта чему-нибудь за этот запуск.

    Ищется в логе, а не спрашивается у сценария: шейдер, не собравшийся, и
    раскладка вершин, не сошедшаяся с ним, не роняют приложение — окно живёт,
    разметка отвечает, а рисуется пустое место. Ни один шаг сценария такого не
    замечает: у шара и канвы просмотра имён нет.
    """
    if not os.path.exists(HOST_LOG):
        return False
    with open(HOST_LOG, encoding="utf-8", errors="replace") as log:
        # По приставке, а не по словам самого отказа: их три рода — проверка,
        # нехватка памяти и внутренняя ошибка, — и печатает их всех один
        # обработчик (`veldcore/platform/host/core/src/setup.rs`). Ловля по
        # «Validation Error» пропустила бы нехватку памяти, которая называет
        # себя иначе.
        return "wgpu: " in log.read()


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
                failed.append(f"{name} (нет файла)")
                continue
            if os.path.exists(WINDOW_STATE):
                os.remove(WINDOW_STATE)
            print(f"  {name}: ", end="", flush=True)
            outcome, spent = play(path, name)
            print(f"{outcome} — {spent:.1f}s")
            if outcome != "сошёлся":
                failed.append(f"{name} ({outcome.lower()})")
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
