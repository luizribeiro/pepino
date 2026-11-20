mod app;
#[allow(dead_code)]
mod batch;
mod projection;
mod protocol;
mod state;

pub use app::App;
pub use state::{LocalState, OpenOptions};

pub use protocol::{
    AppError, AppErrorKind, CommentRecord, Date, EdgeKind, FieldValue, Hlc, Request, Response,
    Status, TaskRecord,
};
