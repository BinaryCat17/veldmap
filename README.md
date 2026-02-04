# VeldMap: Децентрализованная P2P ГИС-платформа

Современная модульная платформа для визуализации ландшафта и работы с геопространственными данными. Архитектура построена на использовании **Veld Universal ABI** для высокопроизводительного взаимодействия между нативным хостом и WASM-плагинами, а также P2P-сети **Iroh** для глобальной синхронизации.

## Ключевые особенности архитектуры

- **Veld Universal ABI**: Кастомный бинарный интерфейс, обеспечивающий zero-copy доступ к памяти и исключающий жесткую зависимость плагинов от рантайма (модули не требуют `extism-pdk`).
- **Unified WGPU**: Единая графическая подсистема на базе WGPU. Доступна как в GUI-клиенте для визуализации, так и в CLI-хостах для headless-вычислений.
- **Resource-based Communication**: Замена передачи сырых байтов на систему `ResourceHandle` (локальный `u64` ID + глобальный `BLAKE3` хеш), позволяющую разделять тяжелые GPU-ресурсы (текстуры, буферы) между модулями без лишнего копирования.

---

## Структура проекта

Проект разделен на три независимых домена:

### 1. [VeldCore](./veldcore) (Ядро платформы)
- **`veldcore/proto`**: Системные интерфейсы (`services.proto`, `ui.proto`).
- **`veldcore/host/gui`**: Графический клиент (WGPU + Winit). Среда для интерактивных ГИС-приложений.
- **`veldcore/host/cli`**: Консольный клиент для вычислительных узлов. Поддерживает WGPU Compute.

### 2. [VeldSDK](./veldsdk) (Инструментарий разработки)
- **`rpc`**: Реализация Veld Universal ABI и макросы инициализации модулей.
- **`core`**: Базовые системные функции (FS, GPU, логирование).
- **`iced`**: Высокоуровневый рантайм для создания интерфейсов на [Iced](https://iced.rs/).

### 3. [VeldGIS](./veldgis) (Прикладное приложение)
- **`veldgis/apps`**: Конечные приложения (Data Browser, Desktop Client).
- **`veldgis/modules`**: Функциональные блоки (Render, Tile Server, Data Provider).
- **`veldgis/proto`**: Прикладные ГИС-протоколы.

---

## Системные сервисы (API)

### 1. Сервис `system` (Доступен везде)
- **`log`**: Запись в системный лог с пробросом в систему трассировки хоста.
- **`fs_read` / `fs_write`**: Работа с файлами. Поддерживается асинхронное чтение напрямую в GPU-ресурсы.
- **`fs_list` / `fs_delete`**: Управление файловой структурой.

### 2. Управление ресурсами (GPU)
Вместо передачи тяжелых массивов `Vec<u8>`, плагины оперируют дескрипторами `ResourceHandle`.
- **`veld_gpu_write`**: Запись данных в GPU-текстуру или буфер.
- **`veld_gpu_read`**: Обратное чтение данных (например, для сохранения скриншотов).

### 3. Сервис `app` (Только GUI)
Позволяет плагинам управлять выводом изображения.
- **`display`**: Принимает `ResourceHandle`, мгновенно отображая содержимое GPU-текстуры в окне приложения.

---

## Разработка плагинов

Плагины VeldMap являются "чистыми" WASM-модулями. SDK предоставляет два уровня абстракции.

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
Используется макрос `define_iced_module!`, который автоматически интегрирует события UI.

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

### Архитектура обработчиков («Free Functions»)
В отличие от классического Iced, здесь используется архитектура свободных функций:
- Логика обновления распределяется по небольшим функциям в `handlers.rs`.
- Каждая функция принимает `&mut LocalState` первым аргументом.
- Обработчики возвращают `Command<Message>` для выполнения побочных эффектов.
- Данные из вариантов `Message` (например, `path` в `DownloadFile(path)`) автоматически распаковываются макросом и передаются в функцию.

```rust
// Пример в handlers.rs
pub fn handle_search(state: &mut LocalState) -> Command<Message> {
    state.status_message = "Searching...".into();
    let req = SearchRequest { query: state.query.clone(), ..Default::default() };
    
    // Используем макрос для лаконичного RPC-вызова через ABI
    rpc_command!("data-provider", "search", req.encode_to_vec(), SearchResponse, Message::SearchResult)
}
```

### Управление асинхронностью (Commands)

VeldMap SDK использует декларативный подход. Обработчики возвращают `Command<M>`, которую рантайм выполняет в фоне через системный мост.

#### Основные инструменты:

1.  **`rpc_command!`**: Самый быстрый способ сделать RPC-запрос. Принимает имя сервиса, метод, данные, тип ответа и обработчик.
    ```rust
    rpc_command!("service", "method", payload, ResponseType, Message::Result)
    ```

2.  **`rpc_call!`**: Макрос для использования внутри `async move` блоков. Выполняет запрос, декодирует Protobuf и возвращает `Result<T, String>`.

3.  **`Command::perform`**: Конструктор для любых асинхронных задач (чтение файлов, работа с GPU).
    ```rust
    Command::perform(async move {
        let data = core::fs_read("file.txt").await?;
        Ok(process(data))
    }, Message::Finished)
    ```

4.  **`yield_now()`**: Принудительная передача управления хосту. Полезна в начале тяжелых задач, чтобы UI успел отрисовать статус "Загрузка...".

---

## Сборка и запуск

Для оркестрации используются Python-скрипты:

- **Сборка всего проекта**: `python3 build.py build`
- **Запуск GUI-версии**: `python3 run-native.py`
- **Очистка**: `python3 build.py clean`

> **Требования**: Rust (stable), Python 3, установленный `wasm32-wasip1` target (`rustup target add wasm32-wasip1`).