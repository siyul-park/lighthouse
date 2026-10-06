/// A thing.
pub struct Thing;

impl Thing {
    pub fn size(&self) -> u32 {
        1
    }
}

/// Makes a thing.
pub fn make() -> Thing {
    Thing
}
