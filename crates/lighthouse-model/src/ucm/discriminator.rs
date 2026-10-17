use super::{Project, Symbol, SymbolId};
use crate::Diagnostic;

impl Project {
    /// The symbol a finding is about, if it is in the finding's file, else the
    /// innermost one whose span holds the start of the finding. `None` for a
    /// finding no symbol covers, such as one about the file as a whole.
    pub fn anchor(&self, finding: &Diagnostic) -> Option<&Symbol> {
        finding
            .symbol
            .as_deref()
            .and_then(SymbolId::parse)
            .and_then(|id| self.symbol(&id))
            .filter(|symbol| symbol.file == finding.file)
            .or_else(|| self.enclosing(finding))
    }

    /// The symbol a finding is about, else the one that encloses or precedes
    /// it; empty when there is none. What tells apart findings that share a
    /// fingerprint ([`Diagnostic::disambiguate`]).
    pub fn discriminator(&self, finding: &Diagnostic) -> String {
        if let Some(symbol) = &finding.symbol {
            return symbol.clone();
        }
        let at = finding.span.start;
        let preceding = self
            .symbols_in(&finding.file)
            .filter(|s| s.span.end < at)
            .max_by_key(|s| s.span.end);
        self.enclosing(finding)
            .or(preceding)
            .map_or_else(String::new, |s| s.id.as_str().to_owned())
    }

    /// The innermost symbol whose span holds the start of the finding.
    fn enclosing(&self, finding: &Diagnostic) -> Option<&Symbol> {
        let at = finding.span.start;
        self.symbols_in(&finding.file)
            .filter(|s| s.span.start <= at && at <= s.span.end)
            .max_by_key(|s| s.span.start)
    }
}
