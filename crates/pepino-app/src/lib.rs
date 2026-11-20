//! Application request and response protocol.
//!
//! The envelope carries [`PROTOCOL_VERSION`]. Fields can be added to response
//! objects at the same version. New request fields must be optional. Receivers
//! reject unknown request fields, so senders need to know that a receiver
//! supports an optional field before sending it. New variants require a new
//! version.

mod app;
mod batch;
mod board;
mod board_requests;
mod catalog;
mod device_grant;
mod device_grant_auth;
mod device_install;
mod device_request;
mod device_update;
mod filter;
mod hlc;
mod identity;
mod initialize;
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

pub use app::{App, SyncStoreError};
pub use board_requests::BoardInfo;
pub use device_grant::{BoardJoinState, DeviceGrant, PeerHint};
pub use device_request::DeviceRequest;
pub use filter::{FilterDiagnostic, FilterInput, FilterPosition, FilterWarning};
pub use hlc::Hlc;
pub use identity::{RosterDevice, RosterState};
pub use log::{
    EventAction, EventChange, EventClassification, EventDetail, EventRecord, InitialTaskFields,
};
pub use pepino_core::filter::keyword_groups as filter_keyword_groups;
pub use state::{LocalBoard, LocalState, OpenOptions};
pub use status::{IdentityRecord, NamedRecord, NetworkStatus, SkippedCounts, StatusCounts};

pub use protocol::{
    AppError, AppErrorKind, CommentRecord, ConflictReference, DependencyRecord, EdgeEffect,
    EdgeExclusionReason, MissingReference, ReadRequest, Request, Response, TaskChange, TaskRecord,
    TaskSummary, TreeNode, WriteRequest,
};

pub use values::{Date, EdgeKind, FieldValue, InvalidDate, Status};
pub use views::SavedView;
pub use wire::{PROTOCOL_VERSION, WireFormat, WireRequest, WireResponse, WireResult};

#[cfg(test)]
extern crate self as pepino_app;

pub use device_grant_auth::SignedGrant;
