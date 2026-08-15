//! Кэш состояния библиотеки — только для отрисовки.
//!
//! Ни каталога, ни суффиксов, ни сидкаров здесь нет и быть не может: всё это
//! знает data-library, а сюда приходит уже выведенный список записей. Наше
//! дело — держать последний присланный список и находить в нём запись.

use crate::proto::data_library::{LibraryEntry, LibraryStatus};

#[derive(Default)]
pub struct LibraryState {
    /// Последнее присланное состояние (data-library/on_state). Целиком
    /// заменяется, а не патчится: библиотека рассылает его при каждом
    /// изменении, и держать здесь свою версию правды было бы вторым
    /// источником истины.
    pub entries: Vec<LibraryEntry>,
}

impl LibraryState {
    /// Запись по ключу провайдера — для экранов Browse/Search, где строка
    /// приходит из каталога провайдера и о диске ничего не знает.
    pub fn by_identifier(&self, identifier: &str) -> Option<&LibraryEntry> {
        if identifier.is_empty() { return None; }
        self.entries.iter().find(|e| e.identifier == identifier)
    }

    /// Имя записи, если этот ключ уже лежит на диске целиком, — то есть чем
    /// открывать: скачанное открывает библиотека, остальное провайдер.
    ///
    /// Правило одно на все пути открытия (просмотр строки, растр наложения):
    /// файл под рукой, и ходить за ним по сети значит ждать того, что уже
    /// есть. Недокачанное сюда не попадает — у `.part` нет ни конца, ни
    /// заголовка, а конвейеру тайлов нужен весь файл.
    pub fn local_name(&self, identifier: &str) -> Option<&str> {
        self.by_identifier(identifier)
            .filter(|entry| status_of(entry) == LibraryStatus::LibComplete)
            .map(|entry| entry.name.as_str())
    }

    /// Сколько записей лежит под этим путём каталога. Так папка сетевого
    /// каталога узнаёт, есть ли у неё что-то на диске: своего ответа на это у
    /// каталога нет, а у библиотеки есть — в ключе каждой записи стоит путь,
    /// откуда её скачали.
    pub fn count_under(&self, prefix: &str) -> usize {
        if prefix.is_empty() { return 0; }
        self.entries.iter().filter(|entry| entry.identifier.starts_with(prefix)).count()
    }

    /// Полнота снимка: сколько его файлов доведено и сколько их в снимке
    /// всего. Второе число ноль — снимок ни разу не обходили, и назвать его
    /// целым не из чего: доведено «всё, что качали», а качали не обязательно
    /// весь снимок.
    ///
    /// Наибольшим из записей, а не первым попавшимся: число приходит извне и
    /// расходится по сидкарам файлов записями на диск, поэтому в момент между
    /// ними часть записей его ещё не знает (0), а знающая — знает верно.
    pub fn snapshot(&self, product: &str) -> (usize, u32) {
        if product.is_empty() { return (0, 0); }
        self.entries
            .iter()
            .filter(|entry| entry.product == product)
            .fold((0, 0), |(done, files), entry| {
                let complete = status_of(entry) == LibraryStatus::LibComplete;
                (done + complete as usize, files.max(entry.siblings))
            })
    }

    /// Сколько байт лежит на диске — считая недокачанное, оно тоже занимает
    /// место.
    pub fn stored(&self) -> u64 {
        self.entries.iter().map(|entry| entry.done).sum()
    }

    /// Идущие закачки: сколько их, сколько сделано и сколько всего. Одним
    /// проходом, потому что показываются они тоже вместе — одной строкой
    /// состояния.
    pub fn downloading(&self) -> (usize, u64, u64) {
        self.entries
            .iter()
            .filter(|entry| status_of(entry) == LibraryStatus::LibDownloading)
            .fold((0, 0, 0), |(count, done, total), entry| {
                (count + 1, done + entry.done, total + entry.total)
            })
    }
}

/// Статус записи как enum, а не как сырой i32 из protobuf.
pub fn status_of(entry: &LibraryEntry) -> LibraryStatus {
    LibraryStatus::try_from(entry.status).unwrap_or(LibraryStatus::LibPaused)
}
