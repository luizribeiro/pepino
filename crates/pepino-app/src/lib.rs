//! Application request and response protocol.
//!
//! The envelope carries [`PROTOCOL_VERSION`]. Fields can be added to response
//! objects at the same version. New request fields must be optional. Receivers
//! reject unknown request fields, so senders need to know that a receiver
//! supports an optional field before sending it. New variants require a new
//! version.

mod app;
mod batch;
mod hlc;
mod projection;
mod protocol;
mod read;
mod state;
mod values;
mod wire;
mod write;

pub use app::App;
pub use hlc::Hlc;
pub use state::{LocalState, OpenOptions};

pub use protocol::{
    AppError, AppErrorKind, CommentRecord, EdgeEffect, EdgeExclusionReason, ReadRequest, Request,
    Response, TaskRecord, WriteRequest,
};

pub use values::{Date, EdgeKind, FieldValue, InvalidDate, Status};
pub use wire::{PROTOCOL_VERSION, WireFormat, WireRequest, WireResponse, WireResult};
