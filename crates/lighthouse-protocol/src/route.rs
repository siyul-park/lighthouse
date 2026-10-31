use crate::{CLASSIFY, EDIT, EXIT, INDEX, INITIALIZE, SHUTDOWN, TRAIN};

/// TypeSafe's System One: the twin of [`CLASSIFY`].
pub const SYSTEM_ONE: Http = Http {
    verb: "POST",
    path: "/v1/systemone",
};

/// OpenAI's fine-tuning job creation: the twin of [`TRAIN`].
pub const FINE_TUNING_JOBS: Http = Http {
    verb: "POST",
    path: "/v1/fine_tuning/jobs",
};

/// Every method of the protocol, in the order docs/plugin-protocol.md lists
/// them. Both transports read it: the stdio loop dispatches these names, and a
/// provider that forwards to an HTTP server posts to these paths.
pub const ROUTES: [Route; 7] = [
    Route {
        method: INITIALIZE,
        http: None,
    },
    Route {
        method: INDEX,
        http: None,
    },
    Route {
        method: CLASSIFY,
        http: Some(SYSTEM_ONE),
    },
    Route {
        method: TRAIN,
        http: Some(FINE_TUNING_JOBS),
    },
    Route {
        method: EDIT,
        http: None,
    },
    Route {
        method: SHUTDOWN,
        http: None,
    },
    Route {
        method: EXIT,
        http: None,
    },
];

/// One method and, when a published HTTP API does the same, its twin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    pub method: &'static str,
    pub http: Option<Http>,
}

/// The HTTP request that does what a method does: the published API the
/// method's body comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Http {
    pub verb: &'static str,
    pub path: &'static str,
}
