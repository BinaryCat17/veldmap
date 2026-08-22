"""Щелчок колеса приближает одинаково и шар, и канву просмотра.

Колесо у них одно, а множители разные: канва получает готовый `factor` и
множит им масштаб, шар получает щелчки и множит ими высоту. Величины поэтому
живут по разные стороны провода — в `data-browser`, который разбирает ввод, и
в камере глобуса, которая знает, во что его превращать, — и свести их в одну
константу нельзя, не отдав шаг колеса модулю, не знающему про мышь.

Разъехаться им ничто не мешает: правящий одну о второй не узнает, а сборка от
этого не покраснеет — для неё это два не связанных числа. Здесь они и
сводятся.
"""
import os
import re

from conftest import PROJECT_ROOT

CANVAS = os.path.join(PROJECT_ROOT, "veldmodules", "data-browser", "src", "handlers", "preview.rs")
GLOBE = os.path.join(PROJECT_ROOT, "veldmodules", "globe", "src", "camera.rs")


def constant(path: str, name: str) -> float:
    """Число, а не текст: `0.8` и `1.0 / 1.25` — один и тот же шаг, и проверка,
    придирающаяся к записи, ловила бы правку там, где её нет."""
    with open(path, encoding="utf-8") as f:
        found = re.search(rf"const {name}: f64 = ([^;]+);", f.read())
    assert found, f"в {os.path.basename(path)} не нашлось {name}"
    written = found.group(1).strip()
    parts = [float(part) for part in written.split("/")]
    assert 1 <= len(parts) <= 2, f"{name} записана не числом и не дробью: {written}"
    return parts[0] if len(parts) == 1 else parts[0] / parts[1]


def test_the_wheel_steps_the_same_everywhere():
    """Шаг у шара — обратный шагу канвы: у канвы больше значит крупнее, у шара
    ближе значит ниже."""
    canvas = constant(CANVAS, "ZOOM_PER_CLICK")
    globe = constant(GLOBE, "ZOOM_PER_STEP")
    assert abs(canvas * globe - 1.0) < 1e-12, (
        f"канва множит масштаб на {canvas}, шар — высоту на {globe}: "
        f"одно колесо, два разных шага"
    )


def test_the_globe_names_the_canvas_constant():
    """Ссылка в камере обязана называть ту самую константу: без имени правящий
    её не найдёт места, которое на неё опирается."""
    with open(GLOBE, encoding="utf-8") as f:
        camera = f.read()
    assert "ZOOM_PER_CLICK" in camera, "камера не называет, с чем сведён её шаг"
    assert "test_wheel_step.py" in camera, "камера не называет проверку, которая их сводит"

    with open(CANVAS, encoding="utf-8") as f:
        canvas = f.read()
    assert "ZOOM_PER_STEP" in canvas, "канва не называет, с чем сведён её шаг"
