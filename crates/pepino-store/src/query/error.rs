use thiserror::Error;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("{term}: {kind}")]
pub struct QueryError {
    pub term: String,
    pub kind: QueryErrorKind,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum QueryErrorKind {
    #[error("no current identity for me")]
    MissingIdentity,
}
