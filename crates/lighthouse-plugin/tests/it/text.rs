use std::{cell::Cell, sync::Arc};

use lighthouse_graph::Error;
use lighthouse_plugin::Text;

#[test]
fn text_lazy_reads_once_when_first_asked_for() {
    let reads = Cell::new(0);
    let text = Text::lazy(|| {
        reads.set(reads.get() + 1);
        Ok(Arc::new(Some("a\nb\n".to_owned())))
    });
    assert_eq!(reads.get(), 0);
    assert_eq!(text.get(), "a\nb\n");
    assert_eq!(text.get(), "a\nb\n");
    assert_eq!(reads.get(), 1);
}

#[test]
fn text_get_is_empty_for_a_file_that_is_no_text() {
    let text = Text::lazy(|| Ok(Arc::new(None)));
    assert_eq!(text.get(), "");
    assert!(text.failure().is_none());
}

#[test]
fn text_failure_tells_why_the_text_could_not_be_read() {
    let text = Text::lazy(|| Err(Error::Incomplete("gone".to_owned())));
    assert!(text.failure().is_none(), "not asked for, not failed");
    assert_eq!(text.get(), "");
    assert!(matches!(text.failure(), Some(Error::Incomplete(_))));
}
