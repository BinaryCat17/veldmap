"""Правила документации, которые держатся механически (docs/decisions/0001).

Область — docs/, CLAUDE.md и README.md: README — указатели на страницы docs/,
и ссылки в нём держатся тем же правилом.

Правила три.

1. Ссылка ведёт на то, что есть в дереве. Путём считается цель
   markdown-ссылки без схемы и без якоря, а также путь в обратных кавычках от
   корня репозитория: начинается с каталога верхнего уровня, содержит `/`,
   без `*` и `<>` (это шаблоны, а не пути). Путь под .gitignore существовать
   не обязан — runtime/logs/, generated/, target/.
2. В тексте нет `file.rs:123`: номер строки стареет раньше кода.
3. Нет путей вне репозитория (`~/`, `/home/`, `/Users/`, `.claude/`).

Исключение одно — docs/roadmap.md: план с датой, называет файлы, которых ещё
нет, и строки, которые ещё не сдвинулись; удаляется по выполнении. На него
не распространяются первые два правила, третье — распространяется.
"""
import os
import re
import subprocess

import pytest

from conftest import PROJECT_ROOT

DOCS_DIR = os.path.join(PROJECT_ROOT, "docs")
PLAN = "docs/roadmap.md"

LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")
BACKTICKED = re.compile(r"`([^`\n]+)`")
PATH_LIKE = re.compile(r"^[A-Za-z0-9_./-]+$")
LINE_REF = re.compile(
    r"[\w./-]+\.(?:rs|py|proto|yaml|md|txt|toml|json|j2|wgsl):\d+")
OUTSIDE = ("~/", "/home/", "/Users/", ".claude/")


def pages() -> list[str]:
    found = ["CLAUDE.md", "README.md"]
    for root, _dirs, files in os.walk(DOCS_DIR):
        for name in sorted(files):
            if name.endswith(".md"):
                found.append(os.path.relpath(os.path.join(root, name), PROJECT_ROOT))
    return sorted(found)


def top_level_dirs() -> set[str]:
    return {name for name in os.listdir(PROJECT_ROOT)
            if os.path.isdir(os.path.join(PROJECT_ROOT, name)) and not name.startswith(".")}


def ignored(paths: list[str]) -> set[str]:
    """Какие из путей лежат под .gitignore — спрашивается у самого git."""
    if not paths:
        return set()
    done = subprocess.run(["git", "check-ignore", "--stdin"],
                          input="\n".join(paths), capture_output=True,
                          text=True, cwd=PROJECT_ROOT, check=False)
    return set(done.stdout.split())


def read(page: str) -> str:
    with open(os.path.join(PROJECT_ROOT, page), encoding="utf-8") as f:
        return f.read()


def link_targets(text: str) -> list[str]:
    targets = []
    for target in LINK.findall(text):
        if target.startswith(("http://", "https://", "mailto:", "#")):
            continue
        targets.append(target.split("#", 1)[0])
    return targets


def backticked_paths(text: str, roots: set[str]) -> list[str]:
    paths = []
    for token in BACKTICKED.findall(text):
        if "/" not in token or not PATH_LIKE.match(token):
            continue
        if token.split("/", 1)[0] in roots:
            paths.append(token)
    return paths


def resolve(page: str, target: str) -> str:
    """Относительно страницы, как читает браузер; от корня — если так нашлось;
    не нашлось нигде — как относительно страницы, чтобы отчёт назвал то место,
    куда ссылка ведёт на самом деле."""
    beside = os.path.normpath(os.path.join(os.path.dirname(page), target))
    if os.path.exists(os.path.join(PROJECT_ROOT, beside)):
        return beside
    from_root = os.path.normpath(target)
    if os.path.exists(os.path.join(PROJECT_ROOT, from_root)):
        return from_root
    return beside


def dangling(page: str, text: str) -> list[str]:
    """Пути со страницы, которых в дереве нет и которые не под .gitignore."""
    roots = top_level_dirs()
    candidates = [resolve(page, t) for t in link_targets(text)]
    candidates += [p.rstrip("/") for p in backticked_paths(text, roots)]
    missing = [p for p in candidates if not os.path.exists(os.path.join(PROJECT_ROOT, p))]
    return sorted(set(missing) - ignored(missing))


@pytest.mark.parametrize("page", [p for p in pages() if p != PLAN])
def test_links_and_paths_lead_into_the_tree(page):
    found = dangling(page, read(page))
    assert found == [], f"{page}: ссылки в никуда: {found}"


@pytest.mark.parametrize("page", [p for p in pages() if p != PLAN])
def test_no_line_references(page):
    found = sorted(set(LINE_REF.findall(read(page))))
    assert found == [], f"{page}: номера строк в прозе: {found}"


@pytest.mark.parametrize("page", pages())
def test_no_paths_outside_the_repository(page):
    text = read(page)
    found = [mark for mark in OUTSIDE if mark in text]
    assert found == [], f"{page}: пути вне репозитория: {found}"


def test_the_rules_see_a_violation():
    """Ослабевшее правило сборку не ломает — поэтому каждое проверяется на
    заведомо плохой странице, и первое — целиком, до вердикта: ссылка в
    никуда красная, несуществующий путь под .gitignore — нет."""
    text = ("см. [страницу](nowhere.md), `docs/nowhere.md`, "
            "`runtime/logs/nowhere.log`, range.rs:28 и ~/.claude/plans")
    assert dangling("docs/probe.md", text) == ["docs/nowhere.md"]
    assert LINE_REF.findall(text) == ["range.rs:28"]
    assert any(mark in text for mark in OUTSIDE)
