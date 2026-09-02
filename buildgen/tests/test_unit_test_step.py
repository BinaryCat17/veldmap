"""Шаг юнит-тестов сборки: что он гоняет и что показывает.

Две чистые функции build.py, ослабев, не сломали бы сборку: парсер
предупреждений, вернувший пустой список, спрятал бы их все, а список пакетов
воркспейса, отставший от Cargo.toml, оставил бы новый крейт без прогона. Здесь
обе проверяются на том, с чем они работают: на консервированном выводе cargo и
на живом воркспейсе.
"""
import os

from conftest import PROJECT_ROOT

import build

CARGO_OUTPUT = """\
   Compiling veldmap-globe v0.1.0
warning: function `only_a_frame_of_parallels_and_meridians_is_axial` is never used
    --> src/../../src/overlay.rs:3846:8
     |
3846 |     fn only_a_frame_of_parallels_and_meridians_is_axial() {
     |        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
     |
     = note: `#[warn(dead_code)]` on by default

warning: duplicated attribute
    --> src/../../src/overlay.rs:3814:5
     |
3814 |     #[test]
     |     ^^^^^^^

warning: `veldmap-globe` (lib test) generated 2 warnings
    Finished `release` profile [optimized] target(s) in 12.3s
     Running unittests src/lib.rs
"""


def test_warnings_are_read_with_their_place():
    found = build.rustc_warnings(CARGO_OUTPUT)
    assert found == [
        "warning: duplicated attribute --> src/../../src/overlay.rs:3814:5",
        "warning: function `only_a_frame_of_parallels_and_meridians_is_axial` is never used "
        "--> src/../../src/overlay.rs:3846:8",
    ]


def test_the_cargo_summary_line_is_not_a_warning():
    """«`crate` generated N warnings» — счёт, а не предупреждение: считая
    его, сводка врала бы числом."""
    assert build.rustc_warnings("warning: `veldmap-globe` (lib test) generated 2 warnings\n") == []
    assert build.rustc_warnings("test result: ok. 3 passed\n") == []


def test_the_workspace_is_asked_from_cargo():
    """Крейт `util` — тот, чьи тесты не бежали, пока список пакетов жил в
    build.py: у cargo его не забыть."""
    packages = build.workspace_packages(os.path.join(PROJECT_ROOT, "veldcore"))
    assert "veldmap-host-util" in packages
    assert "veldsdk" in packages and "veldmap-host-core" in packages
