# VeldMap: Децентрализованная P2P ГИС-платформа

Современная модульная платформа для визуализации ландшафта и работы с геопространственными данными. Архитектура построена на изоляции бизнес-логики в WASM-плагинах и использовании P2P-сети Iroh для синхронизации.

## Структура проекта

Проект разделен на три независимых домена:

### 1. [VeldCore](./veldcore) (Ядро платформы)
Содержит реализации хостов и системные протоколы.
- **`veldcore/proto`**: Системные интерфейсы (`services.proto`, `ui.proto`).
- **`veldcore/host/gui`**: Графический клиент (WGPU). Полноценная среда для интерактивных приложений.
- **`veldcore/host/cli`**: Консольный клиент для headless-узлов.

### 2. [VeldSDK](./veldsdk) (Инструментарий разработки)
Единый SDK для создания плагинов. Находится в `veldsdk/rust`.
- **`rpc`**: Генерация системного моста и макросы инициализации.
- **`core`**: Базовые системные функции (FS, логирование).
- **`iced`**: Высокоуровневый рантайм для создания интерфейсов на [Iced](https://iced.rs/).

### 3. [VeldGIS](./veldgis) (Прикладное приложение)
Пользовательское пространство, превращающее платформу в ГИС.
- **`veldgis/apps`**: Конечные приложения (Data Browser, Desktop Client).
- **`veldgis/modules`**: Функциональные блоки (Render, Tile Server, Data Provider).
- **`veldgis/proto`**: Прикладные ГИС-протоколы.

---

## Системные сервисы (API)

### 1. Сервис `system`
*Доступен во всех типах хостов через Protobuf-интерфейс.*
- **`log`**: Запись в системный лог с указанием уровня (LogLevel).
- **`fs_read`**: Чтение файла с диска хоста.
- **`fs_write`**: Запись файла (автоматически создает директории).
- **`fs_list`**: Получение списка файлов в директории.
- **`fs_delete`**: Удаление файла или директории.

### 2. Сервис `app`
*Доступен ТОЛЬКО в `host-gui`.*
Позволяет плагинам напрямую управлять выводом изображения (используется внутри SDK).
- **`display`**: Обновление изображения (принимает Protobuf `UIDisplayCommand`).

---

## Разработка плагинов

SDK предоставляет два уровня абстракции в зависимости от задач модуля.

### 1. Системные модули (без UI)
Используется макрос `define_module!` для регистрации RPC-обработчиков.

```rust
define_module! {
    config: MyConfig,
    state: MyState,
    init: handlers::module_init,
    handlers: {
        "get_data" => handlers::handle_get : Request => Response,
    }
}
```

### 2. Графические модули (Iced)
Используется макрос `define_iced_module!`, который автоматически мапит события UI на функции-обработчики.

```rust
define_iced_module! {
    config: LocalConfig,
    state: LocalState,
    message: Message,
    init: handlers::module_init,
    view: view::view,
    handlers: {
        SearchPressed => handlers::handle_search,
        DownloadFile(path) => handlers::handle_download,
    }
}
```

#### Архитектура обработчиков («Free Functions»)
В отличие от классического Iced, здесь используется архитектура свободных функций, аналогичная системным модулям:
- Логика обновления не загромождает один большой метод `update`, а распределяется по небольшим функциям в `handlers.rs`.
- Каждая функция принимает `&mut LocalState` первым аргументом.
- Обработчики возвращают `Command<Message>` для выполнения побочных эффектов.
- Данные из вариантов `Message` (например, `path` в `DownloadFile(path)`) автоматически распаковываются макросом и передаются в функцию.

```rust
// Пример в handlers.rs
pub fn handle_download(state: &mut LocalState, path: String) -> Command<Message> {
    state.status_message = format!("Downloading {}...", path);
    Command::perform(async move {
        // Логика работы с файлами или сетью
        Message::DownloadFinished(Ok(path))
    })
}
```

---

## Сборка и запуск

Для удобства оркестрации в корне проекта находятся Python-скрипты:

- **Сборка всего проекта**: `python3 build.py build`
- **Запуск GUI-версии**: `python3 run-native.py`
- **Очистка артефактов**: `python3 build.py clean`

Каждый домен (`veldcore` и `veldgis`) является независимым Rust-воркспейсом и может собираться отдельно.
