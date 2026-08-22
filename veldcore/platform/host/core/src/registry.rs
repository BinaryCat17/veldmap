//! Реестр ресурсов — единственный источник истины о существовании ресурса,
//! его носителе (payload) и правах доступа (lease).
//!
//! Запись одна на ресурс: lease и payload вместе, поэтому регистрация и
//! освобождение атомарны относительно карты. Разносить носители по
//! отдельным картам нельзя — согласовывать их пришлось бы вручную, аллокация
//! стала бы двухшаговой без отката, а освобождение — перебором карт.
//!
//! Акцессоры `payload`/`payload_mut` выполняют замыкание под guard'ом
//! DashMap. Отсюда два правила: нельзя звать их, уже держа guard той же
//! карты (дедлок шарда), и нельзя звать из замыкания обратно в реестр
//! (`register`/`unregister`/`payload*`) — guard берётся рекурсивно. Долгие
//! операции (блокирующее чтение носителя) выполняют, вынеся Arc из
//! замыкания и отпустив guard — так делает `MemoryManager::read`.

use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::memory::DataBacking;

pub type ResourceId = u64;

/// Хост под своим именем. Модульные инстансы нумеруются с единицы
/// (см. `Dispatcher::alloc_instance_id`), так что ноль ничей — им хост и
/// назвался бы, заговори он с реестром от себя.
///
/// Сегодня не говорит: ресурсы упавшего он забирает по владельцу, минуя
/// аренду, а поверхность кадрового цикла берёт без спроса. Ветка держится на
/// будущее — и потому держится в одном месте: разойдись правило по вызывающим,
/// оживший однажды ноль открыл бы не то и не там.
pub const HOST_ID: u32 = 0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

pub struct Lease {
    pub owner_id: u32,
    pub readers: Vec<u32>,
    /// Кому владелец выдал право писать — так владелец окна назначает
    /// рендерера его целевой текстуры. Право писать включает и чтение.
    pub writers: Vec<u32>,
}

impl Lease {
    pub fn new(owner_id: u32) -> Self {
        Self { owner_id, readers: Vec::new(), writers: Vec::new() }
    }

    /// Распоряжается ли этим ресурсом — то есть вправе ли раздавать права и
    /// передавать владение. Владелец и хост, больше никто: пишущий сосед пишет
    /// и освобождает, но раздать полученное дальше не может.
    pub fn owned_by(&self, module_id: u32) -> bool {
        self.owner_id == module_id || module_id == HOST_ID
    }

    pub fn can_read(&self, module_id: u32) -> bool {
        self.owned_by(module_id)
            || self.readers.contains(&module_id)
            || self.writers.contains(&module_id)
    }

    /// Владелец и хост пишут всегда — это и есть владение. Остальные — только
    /// по выданному гранту.
    ///
    /// Срока у права нет: аренды с TTL в платформе не существует. По этому же
    /// праву пускает `veld_resource_free` (см. abi.rs), поэтому любое условие,
    /// временно закрывающее запись, закрыло бы владельцу и освобождение.
    pub fn can_write(&self, module_id: u32) -> bool {
        self.owned_by(module_id) || self.writers.contains(&module_id)
    }

    pub fn add_reader(&mut self, module_id: u32) {
        if !self.readers.contains(&module_id) && module_id != self.owner_id {
            self.readers.push(module_id);
        }
    }

    pub fn add_writer(&mut self, module_id: u32) {
        if !self.writers.contains(&module_id) && module_id != self.owner_id {
            self.writers.push(module_id);
        }
    }

    /// Передать владение целиком: новый владелец и чистые списки.
    ///
    /// Чистые — потому что гранты выданы прежним владельцем и держатся на его
    /// слове. Уцелей они, и отданный ресурс пришёл бы к новому хозяину с
    /// чужими читателями, которых он не звал и снять не может: раздаёт права
    /// только владелец, а этих раздал не он.
    pub fn transfer_to(&mut self, module_id: u32) {
        self.owner_id = module_id;
        self.readers.clear();
        self.writers.clear();
    }
}

/// Непрозрачный GPU-объект: адресуется по id, но байтового диапазона за ним
/// нет — читать со смещением нечего.
///
/// Текстура здесь, а не среди байтовых носителей, именно поэтому: прочитать
/// её по смещению нельзя (копия GPU→CPU остановила бы конвейер), а можно
/// только залить в неё изображение целиком. Байтовый ресурс — тот, у которого
/// работает `read(offset, size)`; по этой границе enum и поделён.
#[derive(Clone)]
pub enum GpuObject {
    Texture { texture: Arc<wgpu::Texture>, width: u32, height: u32, format: i32 },
    /// Размеры и формат — исходной текстуры: кадровый цикл клампит по размерам
    /// viewport и scissor (знать размеры окна для этого ему не нужно), а по
    /// формату отличает цветной аттачмент от буфера глубины.
    ///
    /// `texture` — она же, по id: право писать принадлежит текстуре, а не
    /// сделанному по ней виду. Вид заводит тот, кто собирается им пользоваться,
    /// и владельцем вида становится он же — то есть на самом виде право записи
    /// у него есть всегда. Спрашивать надо у текстуры, иначе выданное право
    /// читать превращается в право писать одним лишним вызовом.
    TextureView {
        view: Arc<wgpu::TextureView>,
        texture: ResourceId,
        width: u32,
        height: u32,
        format: i32,
    },
    Sampler(Arc<wgpu::Sampler>),
    BindGroupLayout(Arc<wgpu::BindGroupLayout>),
    RenderPipeline(Arc<wgpu::RenderPipeline>),
    BindGroup(Arc<wgpu::BindGroup>),
    ShaderModule(Arc<wgpu::ShaderModule>),
}

/// Носитель ресурса: байты, адресуемые смещением, либо непрозрачный
/// GPU-объект. Ресурс один — id, владение (lease) и освобождение у обоих
/// вариантов общие.
pub enum ResourcePayload {
    Data(DataBacking),
    Gpu(GpuObject),
}

pub struct ResourceEntry {
    pub lease: Lease,
    pub payload: ResourcePayload,
}

pub struct ResourceRegistry {
    next_id: AtomicU64,
    entries: DashMap<ResourceId, ResourceEntry>,
}

impl ResourceRegistry {
    pub fn new() -> Self {
        Self {
            // Id 0 обозначает хост (суперпользователь в lease-проверках)
            next_id: AtomicU64::new(1),
            entries: DashMap::new(),
        }
    }

    pub fn register(&self, payload: ResourcePayload, owner_id: u32) -> ResourceId {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let entry = ResourceEntry {
            lease: Lease::new(owner_id),
            payload,
        };
        self.entries.insert(id, entry);
        id
    }

    pub fn unregister(&self, id: ResourceId) -> bool {
        self.entries.remove(&id).is_some()
    }

    /// Освобождает всё, чем владеет этот инстанс, и возвращает, сколько
    /// освободила.
    ///
    /// Это деструкторы убитого, исполненные хостом. Убийство происходит
    /// посреди любой работы и ничего не разматывает — модуль мог остаться
    /// владельцем наполовину собранной текстуры, и вернуть её может только
    /// тот, у кого лежит таблица владения. Ровно так же система забирает
    /// дескрипторы у процесса, которому выключили питание.
    ///
    /// Выданные этому инстансу чужие гранты не трогаем: они принадлежат не
    /// ему, а владельцам своих ресурсов, и те распорядятся ими сами.
    pub fn free_owned_by(&self, owner_id: u32) -> usize {
        let doomed: Vec<ResourceId> = self.entries.iter()
            .filter(|e| e.lease.owner_id == owner_id)
            .map(|e| *e.key())
            .collect();
        doomed.iter().filter(|id| self.unregister(**id)).count()
    }

    pub fn check_access(&self, id: ResourceId, requestor_id: u32, access: Access) -> bool {
        if let Some(entry) = self.entries.get(&id) {
            match access {
                Access::Read => entry.lease.can_read(requestor_id),
                Access::Write => entry.lease.can_write(requestor_id),
            }
        } else {
            false
        }
    }

    pub fn update_lease<F>(&self, id: ResourceId, f: F) -> bool
    where
        F: FnOnce(&mut Lease),
    {
        if let Some(mut entry) = self.entries.get_mut(&id) {
            f(&mut entry.lease);
            true
        } else {
            false
        }
    }

    /// Доступ к носителю под shared-guard'ом карты. Guard живёт до конца
    /// замыкания: возвращать из него можно только клоны/копии, а блокирующие
    /// операции выполнять уже после выхода (см. комментарий к модулю).
    pub fn payload<R>(&self, id: ResourceId, f: impl FnOnce(&ResourcePayload) -> R) -> Option<R> {
        self.entries.get(&id).map(|e| f(&e.payload))
    }

    /// То же по mutable-guard'у — для операций, пишущих в носитель на месте
    /// (CPU-буфер, mapped-диапазон, очередь wgpu).
    pub fn payload_mut<R>(&self, id: ResourceId, f: impl FnOnce(&mut ResourcePayload) -> R) -> Option<R> {
        self.entries.get_mut(&id).map(|mut e| f(&mut e.payload))
    }
}

impl Default for ResourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: u32 = 0;
    const OWNER: u32 = 7;
    const OTHER: u32 = 9;

    /// Владение — это и есть право: владельцу открыто всё без гранта.
    #[test]
    fn the_owner_needs_no_grant() {
        let lease = Lease::new(OWNER);
        assert!(lease.can_read(OWNER));
        assert!(lease.can_write(OWNER));
    }

    /// Ноль открывает всё, и это единственная ветка аренды без вызывающих.
    ///
    /// Инстансы нумеруются с единицы, а хост в реестр от своего имени не
    /// ходит: ресурсы упавшего он забирает по владельцу, минуя аренду
    /// ([`ResourceRegistry::free_owned_by`]), а поверхность кадрового цикла
    /// берёт без спроса. Ветка держится на будущее — и держится именно
    /// тестом: попав однажды в число живых, она станет дырой размером с весь
    /// реестр, и заметить её будет негде.
    #[test]
    fn zero_is_the_host_and_opens_everything() {
        let lease = Lease::new(OWNER);
        assert!(lease.owned_by(HOST));
        assert!(lease.can_read(HOST));
        assert!(lease.can_write(HOST));
    }

    /// Распоряжаться может только владелец: пишущему сосед не начальник.
    ///
    /// Этим же предикатом спрашивают право раздавать гранты и передавать
    /// владение (`lease_op` в abi.rs). Ответь он «да» пишущему, и грант на
    /// запись стал бы полным владением — получивший раздал бы его дальше.
    #[test]
    fn a_writer_does_not_own_what_it_writes() {
        let mut lease = Lease::new(OWNER);
        lease.add_writer(OTHER);
        assert!(lease.can_write(OTHER));
        assert!(!lease.owned_by(OTHER));
    }

    /// Чужому без гранта не открыто ничего.
    #[test]
    fn a_stranger_gets_nothing() {
        let lease = Lease::new(OWNER);
        assert!(!lease.can_read(OTHER));
        assert!(!lease.can_write(OTHER));
    }

    /// Право писать включает чтение, обратное неверно.
    ///
    /// Несимметрично это потому, что вопросы разные: читателю показывают
    /// готовое, а пишущему отдают ресурс в руки — по этому же праву он его и
    /// освободит (см. `veld_resource_free` в abi.rs). Дай мы то же самое за
    /// грант на чтение, и всякий, кому показали снимок, мог бы его снести.
    ///
    /// Раздать права дальше пишущий при этом не может: `lease_op` пускает
    /// только владельца.
    #[test]
    fn writing_implies_reading_but_not_the_other_way() {
        let mut reader = Lease::new(OWNER);
        reader.add_reader(OTHER);
        assert!(reader.can_read(OTHER));
        assert!(!reader.can_write(OTHER));

        let mut writer = Lease::new(OWNER);
        writer.add_writer(OTHER);
        assert!(writer.can_write(OTHER));
        assert!(writer.can_read(OTHER));
    }

    /// Повторная выдача не копит записей, а владельцу грант не выдаётся вовсе.
    ///
    /// Список проверяется перебором на каждый вопрос о праве, поэтому расти
    /// ему нельзя; владельцу же грант не нужен по построению, и запись о нём
    /// означала бы, что право у него откуда-то извне.
    #[test]
    fn granting_twice_adds_one_entry_and_never_to_the_owner() {
        let mut lease = Lease::new(OWNER);
        for _ in 0..3 {
            lease.add_reader(OTHER);
            lease.add_writer(OTHER);
            lease.add_reader(OWNER);
            lease.add_writer(OWNER);
        }
        assert_eq!(lease.readers, vec![OTHER]);
        assert_eq!(lease.writers, vec![OTHER]);
    }

    /// Передача владения уносит все прежние гранты.
    ///
    /// Иначе отданный ресурс пришёл бы к новому хозяину с читателями, которых
    /// он не звал и снять не может: раздаёт права только владелец, а этих
    /// раздал не он.
    #[test]
    fn a_transfer_carries_no_old_grants() {
        let mut lease = Lease::new(OWNER);
        lease.add_reader(OTHER);
        lease.add_writer(OTHER);

        lease.transfer_to(OTHER);

        assert_eq!(lease.owner_id, OTHER);
        assert!(lease.readers.is_empty() && lease.writers.is_empty());
        // Прежний владелец теперь чужой — и права у него ровно чужие.
        assert!(!lease.can_read(OWNER));
        assert!(!lease.can_write(OWNER));
    }

    /// Реестр спрашивает у аренды то же самое и тем же родом права.
    ///
    /// Перепутанные местами ветви `Access` — отказ, который ничего не запретит:
    /// читающему откроется запись. Ни одна проверка аренды этого не заметит,
    /// потому что сама аренда останется верной.
    #[test]
    fn the_registry_asks_the_lease_the_same_question() {
        let registry = ResourceRegistry::new();
        let id = registry.register(
            ResourcePayload::Data(crate::memory::DataBacking::Cpu(Vec::new())),
            OWNER,
        );
        registry.update_lease(id, |lease| lease.add_reader(OTHER));

        assert!(registry.check_access(id, OTHER, Access::Read));
        assert!(!registry.check_access(id, OTHER, Access::Write));
        assert!(registry.check_access(id, OWNER, Access::Write));
    }

    /// Несуществующий ресурс отказывает всем — в том числе хосту.
    ///
    /// Отказом, а не молчанием: по нему `veld_resource_free` и `lease_op`
    /// понимают, что дело не выгорело, — а вот **почему**, из него не видно.
    /// «Не твой» и «такого нет» отвечаются одинаково, и различают их
    /// переспросом через [`ResourceRegistry::update_lease`] (см. abi.rs).
    #[test]
    fn a_missing_resource_refuses_everyone() {
        let registry = ResourceRegistry::new();
        for who in [OWNER, HOST, OTHER] {
            assert!(!registry.check_access(404, who, Access::Read));
            assert!(!registry.check_access(404, who, Access::Write));
        }
        // Тот самый переспрос: замыкание не звалось, значит ресурса нет.
        let mut found = false;
        assert!(!registry.update_lease(404, |_| found = true));
        assert!(!found);
    }
}
