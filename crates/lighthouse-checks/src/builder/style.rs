//! The `style_mix` of a test: whether its file holds tests of both case
//! styles at its nesting level.

use lighthouse_model::ucm::Symbol;

use super::Builder;

impl Builder<'_> {
    /// Whether a test of the other case style (table or scenario) sits at the
    /// same nesting level in the file of `symbol`; false for a symbol that is
    /// no test.
    pub(super) fn style_mix(&self, symbol: &Symbol) -> bool {
        let Some(case) = self.project.test(&symbol.id) else {
            return false;
        };
        self.project
            .symbols_in(&symbol.file)
            .filter_map(|other| self.project.test(&other.id))
            .any(|other| other.nesting == case.nesting && other.style != case.style)
    }
}
