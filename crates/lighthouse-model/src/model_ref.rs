/// A reference to a model, `<provider>/<model>`: the provider the project
/// declares and the model that provider serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelRef<'a> {
    pub provider: &'a str,
    pub model: &'a str,
}

impl<'a> ModelRef<'a> {
    /// Splits `reference` at its first `/`; neither part may be empty.
    pub fn parse(reference: &'a str) -> Option<Self> {
        let (provider, model) = reference.split_once('/')?;
        (!provider.is_empty() && !model.is_empty()).then_some(Self { provider, model })
    }
}
