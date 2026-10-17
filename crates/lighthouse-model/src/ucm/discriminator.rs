use super::Project;
use crate::Diagnostic;

impl Project {
    /// The symbol a finding is about, else the one that encloses or precedes
    /// it; empty when there is none. What tells apart findings that share a
    /// fingerprint ([`Diagnostic::disambiguate`]).
    pub fn discriminator(&self, finding: &Diagnostic) -> String {
        if let Some(symbol) = &finding.symbol {
            return symbol.clone();
        }
        let symbols: Vec<_> = self.symbols_in(&finding.file).collect();
        let at = finding.span.start;
        let innermost = symbols
            .iter()
            .filter(|s| s.span.start <= at && at <= s.span.end)
            .max_by_key(|s| s.span.start);
        let preceding = symbols
            .iter()
            .filter(|s| s.span.end < at)
            .max_by_key(|s| s.span.end);
        innermost
            .or(preceding)
            .map_or_else(String::new, |s| s.id.as_str().to_owned())
    }
}
