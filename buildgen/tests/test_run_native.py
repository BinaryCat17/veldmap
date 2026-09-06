"""Лаунчер под прогоном по сценарию: stderr хоста и код его смерти.

Прогон (`run-uitests.py`) читает панику хоста из stderr лаунчера, а stdout
отправляет в никуда; лаунчер, сливающий stderr хоста в свой stdout, унёс бы
панику туда же, и прогон видел бы «не сошёлся» ни с чего. Код смерти от
сигнала лаунчер отдаёт, как оболочка, — 128+N: по нему прогон называет сигнал.
"""
import importlib.util
import os
import signal
import subprocess
import sys

from conftest import BUILDGEN_DIR


def load_launcher():
    """Сам лаунчер: имя файла с дефисом, обычным import его не взять."""
    path = os.path.join(BUILDGEN_DIR, "run-native.py")
    spec = importlib.util.spec_from_file_location("run_native", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Host:
    """Поддельный процесс хоста: умирает названным статусом."""

    def __init__(self, status: int):
        self.status = status

    def wait(self):
        return self.status

    def send_signal(self, _signum):
        pass


def launch(monkeypatch, status: int) -> tuple[int, dict]:
    """Запуск лаунчера с поддельным хостом: код лаунчера и доводы Popen."""
    launcher = load_launcher()
    seen = {}

    def popen(cmd, **kwargs):
        seen.update(kwargs)
        return Host(status)

    monkeypatch.setattr(subprocess, "Popen", popen)
    monkeypatch.setattr(os.path, "exists", lambda _path: True)
    monkeypatch.setattr(sys, "argv", ["run-native.py"])
    handler = signal.getsignal(signal.SIGTERM)
    try:
        code = launcher.main()
    finally:
        signal.signal(signal.SIGTERM, handler)
    return code, seen


def test_the_hosts_stderr_stays_the_launchers_stderr(monkeypatch):
    """Ни один поток хоста не сливается в другой: stderr хоста — stderr
    лаунчера, и паника доходит до файла прогона.
    """
    _, popen = launch(monkeypatch, 0)

    assert popen.get("stderr") is None, "stderr хоста перенаправлен — паника не дойдёт до прогона"
    assert popen.get("stdout") is None


def test_death_by_signal_is_returned_as_the_shell_would(monkeypatch):
    """Убитый сигналом хост — код 128+N, а не остаток отрицательного числа."""
    launcher = load_launcher()

    assert launcher.exit_code(0) == 0
    assert launcher.exit_code(1) == 1
    assert launcher.exit_code(-signal.SIGABRT) == 134
    assert launcher.exit_code(-signal.SIGSEGV) == 139

    code, _ = launch(monkeypatch, -signal.SIGABRT)
    assert code == 134
