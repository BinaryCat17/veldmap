#!/usr/bin/env python3
import signal
import subprocess
import os
import sys
import argparse

def main():
    parser = argparse.ArgumentParser(description="VeldMap Run Script")
    parser.add_argument("--debug", action="store_true", help="Run debug build")
    parser.add_argument("--config", default="runtime/config", help="Path to config directory")

    # Собираем известные аргументы и все остальные для проброса
    args, extra_args = parser.parse_known_args()
    
    profile_flag = [] if args.debug else ["--release"]
    profile_name = "debug" if args.debug else "release"
    
    print(f"Starting VeldMap Native Runtime ({profile_name})...")
    
    # 1. Запускаем Native Host из воркспейса veldcore
    print(f"-> Launching VeldMap Host ({profile_name})...")
    
    env = os.environ.copy()

    # .env из корня проекта (секреты для подстановки ${VAR} в конфигах)
    # подхватывает сам хост (config::load_dotenv) — так подстановка работает
    # при любом запуске бинарника, а не только через этот лаунчер.

    # Бэкенд GPU здесь не выбирается: раннер собирает wgpu-инстанс с жёстким
    # Vulkan (и с разрешением non-conformant драйверов вроде dzn) — см.
    # runners/desktop/src/main.rs. Переменные WGPU_* он не читает, и флаг
    # выбора бэкенда был бы мёртвым органом управления.

    # Фильтр логов задаётся в runtime/config/core.json (log_filter) — здесь
    # RUST_LOG намеренно не выставляется: заданная переменная перекрывает
    # конфиг целиком, и правки core.json переставали действовать.
    # Для разового переопределения: RUST_LOG=... python3 run-native.py
    # (или строкой в .env).

    # Suppress GPU driver/MESA logs (всё равно лезут в stderr)
    env["EGL_LOG_LEVEL"] = "fatal"
    env["MESA_DEBUG"] = "silent"

    profile_name = "debug" if args.debug else "release"
    binary_path = f"veldcore/target/{profile_name}/veldmap-host-gui"
    
    cmd = [binary_path, "--config", args.config] + extra_args
    
    rust_log = env.get("RUST_LOG")
    print(f"Log filter: {'RUST_LOG=' + rust_log if rust_log else 'из runtime/config/core.json (log_filter)'}")
    
    # Проверяем, существует ли бинарник
    if not os.path.exists(binary_path):
        print(f"Binary not found: {binary_path}")
        print("Please run build first: python3 build.py")
        return 1
    
    # Запускаем процесс напрямую (без cargo). Потоки хоста — наши же, каждый
    # свой: в консоли это консоль, а под прогоном по сценарию stderr — файл,
    # из которого прогон читает панику хоста (run-uitests.py); слитый в stdout
    # он ушёл бы туда же, куда прогон отправляет stdout, — в никуда.
    process = subprocess.Popen(cmd, env=env, stdout=None, stderr=None)

    # TERM, пришедший нам, — хосту: иначе убитый `timeout` или пределом
    # прогона посредник оставляет окно жить, и следующий запуск делит с ним
    # runtime/ и видеокарту. Своего обработчика у хоста нет, он умирает сразу;
    # дожидаемся его, чтобы вернуть его код, а не свой.
    def hand_over(signum, _frame):
        process.send_signal(signum)
    signal.signal(signal.SIGTERM, hand_over)

    try:
        # Код возврата хоста — наш собственный: по нему прогон по сценарию
        # отличает «сошлось» от «не сошлось», и глотать его нельзя.
        return exit_code(process.wait())
    except KeyboardInterrupt:
        print("\nShutting down.")
        process.wait()
        return 130


def exit_code(status: int) -> int:
    """Код возврата хоста, каким его отдаёт оболочка.

    Убитый сигналом процесс Python называет отрицательным номером сигнала, а
    `sys.exit` с таким числом даёт остаток от деления на 256 — код без смысла.
    Оболочка называет ту же смерть кодом 128+N; так же и здесь, и прогон по
    сценарию узнаёт по коду сигнал: `SIGABRT` — паника хоста с
    `panic = "abort"`, `SIGSEGV` — падение, `SIGKILL` — убийство снаружи.
    """
    return status if status >= 0 else 128 - status

if __name__ == "__main__":
    sys.exit(main() or 0)
