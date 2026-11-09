//! The text of the focused file, read when a rule asks for it: a rule that
//! judges what the file declares never asks, so an edit that moves only the
//! text leaves its findings as they were.

use std::{
    cell::{OnceCell, RefCell},
    sync::Arc,
};

use lighthouse_graph::Error;

/// The text of a file, or none when it is no text. Reading it is a dependency
/// of whatever read it.
pub struct Text<'a> {
    load: Load<'a>,
    held: OnceCell<Arc<Option<String>>>,
    failed: RefCell<Option<Error>>,
}

type Load<'a> = Box<dyn Fn() -> Result<Arc<Option<String>>, Error> + 'a>;

impl<'a> Text<'a> {
    /// The text that `load` reads, when it is first asked for.
    pub fn lazy(load: impl Fn() -> Result<Arc<Option<String>>, Error> + 'a) -> Self {
        Self {
            load: Box::new(load),
            held: OnceCell::new(),
            failed: RefCell::new(None),
        }
    }

    /// `text`, which needs no reading.
    pub fn given(text: &str) -> Self {
        let text = Arc::new(Some(text.to_owned()));
        Self::lazy(move || Ok(Arc::clone(&text)))
    }

    /// The text, empty when the file is no text or could not be read; see
    /// [`Text::failure`].
    pub fn get(&self) -> &str {
        let held = self.held.get_or_init(|| match (self.load)() {
            Ok(text) => text,
            Err(error) => {
                self.failed.borrow_mut().get_or_insert(error);
                Arc::new(None)
            }
        });
        held.as_deref().unwrap_or_default()
    }

    /// Why the text could not be read, if it was asked for and could not.
    pub fn failure(&self) -> Option<Error> {
        self.failed.borrow_mut().take()
    }
}
