//! Whether the check or the fix of a decision can run here, decided in one
//! place for whoever asks: a fix run that declines what cannot run, and the
//! descriptor that tells a person beforehand. What a project and a machine
//! allow is the same for a rule, a command, a plugin and a model.

use lighthouse_model::{ModelRef, Unavailable};
use lighthouse_process::sandbox_available;
use lighthouse_spec::{ModelProvider, Models, Task};

use super::Engine;

/// What a check or a fix needs in order to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Needs {
    /// Nothing but this machine: a built-in or CEL check, an ops fix.
    Nothing,
    /// A program the project names: a command check or fix.
    Program,
    /// A program of the bundled catalog, which is Lighthouse's own: it needs
    /// no trust.
    Bundled,
    /// The models bound to a decision for a task.
    Model { decision: String, task: Task },
    /// Nothing serves it yet: a fix of kind `rpc`.
    Unserved,
}

/// What the project and the machine allow: the trust the project was given,
/// whether programs run in a sandbox, and the models it binds.
pub struct Availability<'a> {
    trusted: bool,
    sandboxed: bool,
    models: &'a Models,
}

impl Engine {
    /// What this engine can run.
    pub fn availability(&self) -> Availability<'_> {
        Availability::new(self.trusted, self.ws.sandbox.enabled, &self.binding)
    }
}

impl<'a> Availability<'a> {
    /// `trusted`: the user trusted the project to run the programs it names.
    /// `sandboxed`: `[sandbox] enabled`, so the sandbox must work here.
    pub fn new(trusted: bool, sandboxed: bool, models: &'a Models) -> Self {
        Self {
            trusted,
            sandboxed,
            models,
        }
    }

    /// Whether what `needs` names can run, and if not, why. A route of
    /// models runs when one of its models does.
    pub fn of(&self, needs: &Needs) -> Result<(), Unavailable> {
        match needs {
            Needs::Nothing => Ok(()),
            Needs::Program => self.program(),
            Needs::Bundled => self.sandbox(),
            Needs::Model { decision, task } => self.route(decision, *task),
            Needs::Unserved => Err(Unavailable::NoProvider),
        }
    }

    /// A program runs in a trusted project, and in a sandbox that works when
    /// the project asks for one.
    fn program(&self) -> Result<(), Unavailable> {
        if !self.trusted {
            return Err(Unavailable::UntrustedProject);
        }
        self.sandbox()
    }

    /// A sandbox that works, when the project asks for one.
    fn sandbox(&self) -> Result<(), Unavailable> {
        if self.sandboxed && sandbox_available().is_err() {
            return Err(Unavailable::NoSandbox);
        }
        Ok(())
    }

    fn route(&self, decision: &str, task: Task) -> Result<(), Unavailable> {
        let (route, none) = match task {
            Task::Agent => (self.models.agent_for(decision), Unavailable::NoAgentBinding),
            Task::Classification => (
                self.models
                    .route(decision)
                    .unwrap_or(&self.models.classification),
                Unavailable::NoProvider,
            ),
        };
        let mut first = None;
        for reference in route {
            match self.member(reference) {
                Ok(()) => return Ok(()),
                Err(why) => {
                    first.get_or_insert(why);
                }
            }
        }
        Err(first.unwrap_or(none))
    }

    /// One model of a route: its provider exists, a budget pays for it if it
    /// costs, and a program that is a model runs like any program.
    fn member(&self, reference: &str) -> Result<(), Unavailable> {
        let provider = ModelRef::parse(reference)
            .and_then(|model| self.models.provider(model.provider))
            .ok_or(Unavailable::NoProvider)?;
        let (costs, program) = match provider {
            ModelProvider::Command { cost, .. } => (cost > 0.0, true),
            ModelProvider::Rpc { cost, .. } => (cost != Some(0.0), false),
        };
        if costs && self.models.budget == 0 {
            return Err(Unavailable::NoBudget);
        }
        if program { self.program() } else { Ok(()) }
    }
}
