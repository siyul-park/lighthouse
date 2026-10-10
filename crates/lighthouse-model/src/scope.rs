/// What a rule or an analyzer looks at in one run; a decision's subject maps
/// onto it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RunScope {
    /// Once per file; the focused file is set.
    File,
    /// Once per run over the merged project; there is no focused file.
    Project,
}
