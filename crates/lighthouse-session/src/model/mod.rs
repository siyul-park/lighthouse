//! The models of a project: the ones its decisions are bound to, and what is
//! done with them: grading a check against labels, training a model on them,
//! making a missing model again and attaching one to a decision.

mod attach;
mod bound;
mod eval;
mod examples;
mod labelling;
mod rebuild;
mod train;

pub use attach::{Attachment, Detachment, attach, detach};
pub(crate) use bound::attached;
pub use bound::{Binding, BoundModel, bound_models, remote_bound};
pub use eval::{EvalRequest, eval};
pub use labelling::{Labelling, Repo};
pub use lighthouse_ml::{
    evaluate::{EvalReport, Side},
    metrics::Rate,
    train::Trained,
};
pub use rebuild::{RebuildRequest, Rebuilt, rebuild};
pub use train::{TrainRequest, train};
