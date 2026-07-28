# VeldMap

Настольное приложение для работы со спутниковыми снимками: обход и поиск по
каталогу провайдера (Copernicus Data Space), скачивание продуктов в локальную
библиотеку и просмотр превью — в том числе снимков, которые не скачаны, прямо
из удалённого хранилища.

Построено как микроядро: нативный хост на Rust с рантаймом WebAssembly, а вся
прикладная логика — изолированные wasm-плагины, общающиеся через шину событий.

---

## Требования

| | |
|---|---|
| Rust | nightly (закреплён в `rust-toolchain.toml`), компонент `rust-src` |
| Цели сборки | `wasm32-wasip1` для модулей, `x86_64-unknown-linux-gnu` для хоста |
| Python | 3.10+ (кодогенерация и сборочные скрипты) |
| GPU | Vulkan |
| Линковка (Linux) | `clang` + `mold` |
| Кросс-сборка под Windows | `x86_64-w64-mingw32-gcc` |

Виртуальное окружение для кодогенерации (`buildgen/.venv`, pyyaml + jinja2)
создаётся автоматически при первой сборке.

---

## Сборка и запуск

```bash
python3 buildgen/build.py          # релизная сборка: кодоген + все модули + хост
python3 buildgen/run-native.py     # запуск
```

Сборка проходит три этапа: генерация `generated/`-крейтов из схем, компиляция
wasm-модулей в `build/plugins/`, компиляция нативного хоста.

Прочие команды:

```bash
python3 buildgen/build.py --debug                       # отладочная сборка
python3 buildgen/build.py clean                         # удалить все артефакты
python3 buildgen/build.py --windows --dist-dir <путь>   # кросс-сборка + деплой
python3 buildgen/run-native.py --debug                  # запуск отладочной сборки
python3 buildgen/run-native.py --backend gl             # другой бэкенд wgpu
python3 buildgen/run-native.py --config <каталог>       # другой каталог конфигов
```

Логи пишутся в `runtime/logs/`: `host.log` — то, что видно в консоли,
`trace.log` — полный поток. Оба перезаписываются при каждом запуске.

---

## Структура репозитория

```
veldcore/
  interface/            протокол платформы: .proto + .schema.yaml
    core.proto            ResourceHandle, ResourceOpened, EventEnvelope
    graphics.proto        аргументы графических ABI-вызовов
    modules/<имя>/        контракт платформенного сервиса: .proto + .schema.yaml
  sdk/rust/             veldsdk — SDK модуля: ABI, шина, ресурсы, графика
  platform/host/
    core/                 рантайм: реестр ресурсов, память, шина, задачи, graphics, ABI
    util/                 API для авторов нативных модулей хоста
    generated/            биндинги платформенных контрактов (кодоген)
    modules/<имя>/        нативная реализация платформенного сервиса
    runners/desktop/      цикл событий ОС, окно, кадровый цикл
veldmodules/<имя>/      wasm-модуль: schema.yaml, config.yaml, types.proto, src/
buildgen/               кодогенерация и сборочные скрипты
runtime/                конфиги, шрифты, данные, логи
build/plugins/          собранные .wasm
```

У каждого модуля есть каталог `generated/` — он создаётся кодогенератором и
в него не пишут руками.

---

## Архитектура

### Шина событий

Единственная форма общения между сервисами — публикация события,
fire-and-forget. Синхронных вызовов между модулями нет.

- Топик: `<сервис>/<топик>`, payload — protobuf-сообщение.
- Доставка по умолчанию — всем подписчикам топика. Топик, помеченный в схеме
  `targeted: true`, доставляется только одному адресату, названному по имени.
- Ответ на запрос — это ещё одно событие; отправитель сопоставляет его со своим
  запросом по полю `correlation_id`.
- Каждый подписчик — актор с собственной очередью: его обработчики выполняются
  последовательно, в порядке публикации.
- Хост подписывает каждое доставленное событие именем отправителя; модуль читает
  его через `veldsdk::event_publisher()`.

### ABI

Обращения в состояние хоста идут не через шину, а прямыми синхронными вызовами.

| Группа | Функции |
|---|---|
| Шина и логи | `veld_host_publish`, `veld_host_log` |
| Система | `veld_get_config`, `veld_random_bytes` |
| Память | `veld_memory_alloc_cpu`, `veld_memory_alloc_buffer`, `veld_memory_alloc_texture`, `veld_memory_read`, `veld_memory_write`, `veld_memory_texture_size`, `veld_memory_free` |
| Права | `veld_memory_transfer`, `veld_memory_grant_read`, `veld_memory_grant_write`, `veld_memory_revoke` |
| Графика | `veld_graphics_create_resource`, `veld_graphics_execute` |
| Задачи | `veld_task_alive` |
| Контекст вызова | `veld_input_len`, `veld_input_copy`, `veld_output_set` |

Каждый модуль экспортирует наружу: `init`, `handle_event`, `get_subscriptions`,
`get_service_name`, `veld_alloc`, `veld_free_wasm`. Имя сервиса на шине хост
берёт из `get_service_name` самого бинарника.

### Схема как источник истины

Топики объявляются только в `schema.yaml`. По ней кодогенератор
(`buildgen/generate.py`) собирает крейт `generated/`:

- диспетчер `handle_event`: топик → обработчик с типом payload из схемы;
- список подписок для `get_subscriptions`;
- типизированные стабы `crate::emit::*` для своих выходов и
  `crate::calls::<сервис>::*` для объявленных зависимостей.

Строковых топиков в коде модулей нет. Перед генерацией схема проверяется:
существование каждого типа, наличие топика у сервиса-производителя, совпадение
типов на обеих сторонах, наличие `correlation_id` у пары запрос/ответ
(`replies_to`), совпадение `name:` схемы с именем каталога.

Структура схемы:

```yaml
name: data-library
interface:
  inputs:                       # топики, которые принимает сервис
    on_download: { type: module/DownloadRequest }
  outputs:                      # топики, которые он публикует
    on_state:   { type: module/LibraryState }
    on_open_result:
      type: core/ResourceOpened
      replies_to: on_open       # ответ на свой же вход
dependencies:
  fs:
    subs: [on_read_result]      # чужие выходы, на которые подписываемся
    calls: [on_read]            # чужие входы, в которые публикуем
hooks: [hook_event]             # опциональные хуки жизненного цикла
```

`module/` в типе ссылается на `types.proto` самого модуля, остальные префиксы —
на пакеты из `veldcore/interface/`.

Рукописная часть модуля — `src/module.rs`: он объявляет `Config`, `State`,
`hook_init` и свободные функции-обработчики, чьи имена совпадают с ключами
топиков в схеме. Хук `hook_event` вызывается после каждого обработанного
события.

### Ресурсы и права

Ресурс — это область данных на стороне хоста, адресуемая `ResourceHandle
{ id, size }`. Байты за ним могут лежать в разном:

| Носитель | Что это |
|---|---|
| `Cpu` | обычная память хоста |
| `Range` | файл на диске или удалённый файл, читаемый HTTP Range-запросами |
| `Buffer` | буфер GPU |
| `Texture` | текстура GPU |

Чтение и запись идут по смещению и копируют только запрошенный диапазон в
память вызывающего. Для `Range`-ресурсов это означает, что гигабайтный файл
читается окнами и целиком в память не поднимается; `veldsdk::ResourceReader`
даёт поверх этого обычные `Read + Seek` окнами по 256 КБ, пригодные для любого
парсера.

У каждого ресурса есть аренда: владелец, список читателей и список писателей.
Владелец может передать владение (`transfer`), выдать право чтения или записи
(`grant_read`, `grant_write`), снять все выданные права (`revoke`) и
освободить ресурс (`free`). Проверки выполняет хост на каждом обращении.

Ответ на «открой мне это» у всех одинаковый — `core.ResourceOpened`; его
публикуют `fs`, `network`, `data-provider` и `data-library`. Владение открытым
ресурсом переходит к заказчику. Общая часть этого обмена — в
`veldsdk::resource`: `requester`, `accept`, `hand_off`, `opened`, `discard`.

### Фоновые задачи

Долгие операции регистрируются в реестре задач платформы. Владельцем задачи
становится тот, кто опубликовал породивший её запрос; отменить задачу может
владелец, сервис с выданным ему правом (`tasks/on_grant`) или сам хост.

Жизненный цикл доставляется событиями `tasks/on_task_started` и
`tasks/on_task_finished`; терминальное событие гарантируется для любой
зарегистрированной задачи — при успехе, ошибке и отмене.

Исполнитель, занятый длинной работой внутри одного обработчика, узнаёт об
отмене опросом: `veldsdk::Cancellation::watch(task_id)` поверх
`veld_task_alive`.

### Окно и графика

Окно объявляет модуль-владелец — ключом `window` в своём конфиге. Дальше:

1. Раннер публикует `app/on_window_resized` владельцу.
2. Владелец выделяет текстуру нужного размера, выдаёт `grant_write` рендереру
   и аттачит текстуру хосту через `app/on_set_surface`.
3. Модули записывают render-команды через графический ABI; кадровый цикл
   раннера выполняет их в текстуры-цели и блитит поверхность окна в свопчейн.
4. Ввод и кадровые тики раннер публикует в `app/on_ui_event`.

`ui-service` — отдельный wasm-модуль: принимает разметку (`on_set_view`),
раскладывает её через iced, шейпит текст через cosmic-text в глифовый атлас,
собирает вершины и отправляет их графическим ABI в делегированную ему текстуру.
События виджетов возвращаются владельцу разметки адресным `on_ui_event`.

---

## Сервисы

### Платформенные (нативные, внутри хоста)

| Сервис | Входы | Выходы |
|---|---|---|
| `app` | `on_set_surface` | `on_ui_event`, `on_window_resized`, `on_ready` |
| `fs` | `on_read`, `on_write`, `on_list`, `on_delete` | `on_read_result`, `on_write_result`, `on_list_result`, `on_delete_result` |
| `network` | `on_fs_download`, `on_http`, `on_open` | `on_fs_download_result`, `on_fs_download_progress`, `on_http_result`, `on_open_result` |
| `tasks` | `on_begin`, `on_end`, `on_cancel`, `on_grant` | `on_task_started`, `on_task_finished` |

Набор нативных модулей, входящих в сборку, задаётся в
`platform/host/runners/<раннер>/runner.yaml`.

### Прикладные (wasm)

| Модуль | Роль | Входы | Выходы |
|---|---|---|---|
| `data-browser` | UI приложения: экраны Search, Browse, Downloaded, Preview. Владелец окна | — | — |
| `data-library` | Реестр приобретённого: каталог скачанного, состояние загрузок, раскладка хранения | `on_list`, `on_download`, `on_cancel`, `on_delete`, `on_open` | `on_state`, `on_open_result` |
| `data-provider` | Доступ к хранилищу Copernicus: обход каталога и подпись адресов | `on_sign`, `on_list_path`, `on_search`, `on_open` | `on_signed`, `on_list_path_result`, `on_search_result`, `on_open_result` |
| `image-loader` | Ресурс с изображением → GPU-текстура превью | `on_load` | `on_load_result` |
| `ui-service` | Рендерер разметки в делегированную текстуру | `on_set_view`, `on_set_surface` | `on_ui_event` |

`data-browser` собственных входов не имеет: он общается с остальными только как
потребитель и получает события своих виджетов адресным `ui-service/on_ui_event`.

Поддерживаемые форматы превью: PNG, TIFF, JPEG (потоковое декодирование с
даунсемплом), GIF, BMP, WebP.

### Поток скачивания

```
data-browser  --on_download-->  data-library
data-library  --on_sign------>  data-provider   (подписать адрес)
data-provider --on_signed---->  data-library
data-library  --on_fs_download->  network       (владелец задачи — data-library)
network       --on_fs_download_progress-->  data-library
network       --on_fs_download_result---->  data-library
data-library  --on_state----->  data-browser
```

Отмена: `data-library` публикует `tasks/on_cancel` напрямую. Раскладку хранения
знает только `data-library`; наружу уходят выведенные записи каталога, без
путей и служебных суффиксов.

---

## Конфигурация

Конфиги лежат в `runtime/config/`:

| Файл | Назначение |
|---|---|
| `services.json` | каталог плагинов, путь к логам |
| `core.json` | фильтры логирования, подавление повторов |
| `<имя модуля>.json` | конфиг конкретного модуля |

Конфиг модуля ищется по имени, которое модуль сообщил через
`get_service_name`. Он целиком уезжает в `init` модуля и разбирается его типом
`Config`. Значения вида `${VAR}` подставляются из окружения; `run-native.py`
дополнительно подхватывает `.env` из корня проекта.

Ключ `window` в конфиге модуля объявляет окно:

```json
{
  "window": { "title": "VeldMap GIS", "width": 2048, "height": 1536, "ui_scale": 2.0 }
}
```

Фильтры логов — синтаксис `env_logger`, таргет записи имеет вид
`veldmap::<компонент>::<подсистема>`, где компонент — `host` или имя плагина.
`RUST_LOG` переопределяет `log_filter` из `core.json`.

---

## Добавление wasm-модуля

1. Создать `veldmodules/<имя>/` с файлами `schema.yaml` (`name:` совпадает с
   именем каталога) и `config.yaml` (имя пакета и зависимости Cargo).
2. При наличии собственных типов — добавить `types.proto`; на них ссылаются
   как `module/<Сообщение>`.
3. Написать `src/module.rs`: `Config`, `State`, `hook_init` и по функции на
   каждый топик из `interface.inputs` и `dependencies.*.subs`.
4. Добавить `runtime/config/<имя>.json`.
5. Собрать: `python3 buildgen/build.py`.

Модуль обнаруживается по наличию `schema.yaml` и `config.yaml`; порядок сборки
выводится из зависимостей в схемах. Регистрировать его где-либо ещё не нужно.

Модуль с собственным `types.proto` дополнительно получает wrap-крейт, через
который его типы видны потребителям. Рукописные хелперы для потребителей
кладутся в `wraps/rust/src/wrap.rs`.

---

## Известные ограничения

- Поиск по каталогу (`data-provider/on_search`) не реализован: топик объявлен,
  обработчик — заглушка, ответ `on_search_result` не публикуется. Экран Search
  остаётся в состоянии «Searching...». Он же открывается при старте
  приложения; обход каталога доступен на экране Browse.
- Десктопный раннер поддерживает ровно одно окно: если окно объявляют
  несколько модулей, запуск прерывается с ошибкой.
- Выбор GPU: перебираются только Vulkan-адаптеры, программные отбрасываются.
- Просмотр удалённых (нескачанных) снимков требует, чтобы сервер отвечал на
  Range-запросы, а формат допускал произвольный доступ.

## Лицензия

См. [LICENSE](LICENSE).
