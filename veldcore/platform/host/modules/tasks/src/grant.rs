//! Делегирование права отмены (топик tasks/grant): владелец задачи разрешает
//! другому сервису отменять её — та же семантика, что veld_memory_grant_write
//! у ресурсов (получатель адресуется по имени, резолвится в instance id).

use super::State;
use veldmap_host_util::bindings::proto::tasks::TaskGrantRequest;

pub fn on_grant(state: &State, req: TaskGrantRequest, requestor_id: u32) {
    state.tasks.grant(&req.task_id, requestor_id, &req.service);
}
