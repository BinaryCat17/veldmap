# VeldMap

Настольное приложение для работы со спутниковыми снимками: обход и поиск по
каталогу провайдера (Copernicus Data Space), скачивание продуктов в локальную
библиотеку и просмотр — в том числе снимков, которые не скачаны, прямо из
удалённого хранилища. Построено как микроядро: нативный хост на Rust с
рантаймом WebAssembly, а вся прикладная логика — изолированные wasm-модули,
общающиеся через шину событий.

## Требования

| | |
|---|---|
| Rust | nightly (закреплён в `veldcore/rust-toolchain.toml`) — ради `-Zthreads` в `veldcore/.cargo/config.toml`; нестабильных возможностей языка в коде нет |
| Цели сборки | `wasm32-wasip1` для модулей, `x86_64-unknown-linux-gnu` для хоста |
| Python | 3.10+ (кодогенерация и сборочные скрипты; `buildgen/.venv` создаётся сам) |
| GPU | Vulkan |
| Линковка (Linux) | `clang` + `mold` |
| Кросс-сборка под Windows | `x86_64-w64-mingw32-gcc` |

## Сборка и запуск

```bash
python3 buildgen/build.py          # единственная команда сборки и проверки
python3 buildgen/run-native.py     # запуск
```

Что делает сборка по шагам, как гонять тесты порознь, прогон сценариев
интерфейса — [docs/operations/](docs/operations/build.md). Процесс работы над
кодом — [CLAUDE.md](CLAUDE.md).

## Доступ к Copernicus

Каталог и скачивание требуют ключей S3 к Copernicus Data Space: скопируйте
[`.env.example`](.env.example) в `.env` и впишите свои
(`COPERNICUS_ACCESS_KEY`, `COPERNICUS_ACCESS_SECRET`). Без них сборка и
запуск проходят, поиск по каталогу работает, а листинг хранилища, показ по
сети и скачивание отвечают отказом. Как значения попадают в конфиги —
[docs/operations/configuration.md](docs/operations/configuration.md).

## Документация

- [docs/architecture/overview.md](docs/architecture/overview.md) — карта
  репозитория, микроядро, сервисы и указатели на остальные страницы.
- [docs/glossary.md](docs/glossary.md) — термины; [docs/limitations.md](docs/limitations.md)
  — чего приложение не делает и почему; [docs/decisions/](docs/decisions/README.md)
  — принятые решения с замерами.

## Лицензия

См. [LICENSE](LICENSE).
