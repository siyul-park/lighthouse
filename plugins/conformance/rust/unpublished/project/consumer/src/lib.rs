use shared::Handle;

pub fn identify(handle: &Handle) -> u32 {
    handle.id()
}
