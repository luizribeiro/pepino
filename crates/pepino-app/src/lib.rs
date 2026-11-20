//! Application request and response protocol.
//!
//! The envelope carries [`PROTOCOL_VERSION`]. Fields can be added to response
//! objects at the same version. New request fields must be optional. Receivers
//! reject unknown request fields, so senders need to know that a receiver
//! supports an optional field before sending it. New variants require a new
//! version.

mod app;
mod batch;
mod filter;
mod hlc;
mod list;
mod log;
mod projection;
mod protocol;
mod read;
mod state;
mod status;
mod values;
mod views;
mod wire;
mod write;

pub use app::App;
pub use filter::{FilterDiagnostic, FilterInput, FilterPosition};
pub use hlc::Hlc;
pub use log::{EventChange, EventClassification, EventRecord, InitialTaskFields};
pub use state::{LocalState, OpenOptions};
pub use status::{IdentityRecord, NamedRecord, SkippedCounts, StatusCounts};

pub use protocol::{
    AppError, AppErrorKind, CommentRecord, DependencyRecord, EdgeEffect, EdgeExclusionReason,
    MissingReference, ReadRequest, Request, Response, TaskRecord, TreeNode, WriteRequest,
};

pub use values::{Date, EdgeKind, FieldValue, InvalidDate, Status};
pub use views::SavedView;
pub use wire::{PROTOCOL_VERSION, WireFormat, WireRequest, WireResponse, WireResult};
