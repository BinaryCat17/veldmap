"""Запись сгенерированных файлов.

Кодогенерация идёт на каждой сборке и на неизменной схеме даёт байт в байт тот
же вывод. Но cargo сверяет исходники по mtime, а не по содержимому: перезапись
тем же текстом — для него правка, и крейт пересобирается целиком. Поэтому
`write_if_changed` обязан не трогать файл, содержимое которого не изменилось, —
и обязан тронуть тот, который изменился.
"""


def test_creates_a_missing_file(gen, tmp_path):
    path = tmp_path / "sub" / "lib.rs"
    assert gen.write_if_changed(str(path), "текст") is True
    assert path.read_text() == "текст"


def test_identical_content_leaves_mtime_untouched(gen, tmp_path):
    # Тот самый случай, ради которого хелпер и существует: пустая сборка не
    # должна выглядеть для cargo как правка всех модулей сразу.
    path = tmp_path / "lib.rs"
    gen.write_if_changed(str(path), "текст")
    before = path.stat().st_mtime_ns

    assert gen.write_if_changed(str(path), "текст") is False
    assert path.stat().st_mtime_ns == before


def test_changed_content_is_written(gen, tmp_path):
    path = tmp_path / "lib.rs"
    gen.write_if_changed(str(path), "старое")

    assert gen.write_if_changed(str(path), "новое") is True
    assert path.read_text() == "новое"
