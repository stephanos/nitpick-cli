#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HostClientError {
    #[error("nitpick-agent-host unavailable at {addr}: {message}")]
    Unavailable { addr: String, message: String },
    #[error("unexpected host status: {status}")]
    HttpStatus { status: String },
    #[error("unexpected host status: {status}: {body}")]
    HttpStatusWithBody { status: String, body: String },
    #[error("read nitpick-agent-host response from {addr}{path}: {message}")]
    ReadResponse {
        addr: String,
        path: String,
        message: String,
    },
    #[error("invalid host response at {path}: {message}")]
    InvalidJson { path: String, message: String },
    #[error("serialize host request: {message}")]
    SerializeRequest { message: String },
    #[error("GET host request cannot include a body")]
    GetWithBody,
    #[error("unsupported host request method: {method}")]
    UnsupportedMethod { method: String },
}

impl HostClientError {
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable { .. })
    }
}

impl From<HostClientError> for String {
    fn from(error: HostClientError) -> Self {
        error.to_string()
    }
}

pub type HostClientResult<T> = Result<T, HostClientError>;
