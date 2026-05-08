use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("automerge: {0}")]
    Automerge(#[from] automerge::AutomergeError),

    #[error("automerge load: {0}")]
    AutomergeLoad(#[from] automerge::LoadChangeError),

    #[error("invalid path: {0}")]
    InvalidPath(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("already exists: {0}")]
    AlreadyExists(String),

    #[error("auth failed: {0}")]
    Auth(String),

    #[error("config: {0}")]
    Config(String),

    #[error("protocol: {0}")]
    Protocol(String),

    #[error("network: {0}")]
    Network(String),

    #[cfg(not(target_arch = "wasm32"))]
    #[error("notify: {0}")]
    Notify(#[from] notify::Error),

    #[error("serde json: {0}")]
    SerdeJson(#[from] serde_json::Error),

    #[error("msgpack encode: {0}")]
    MsgpackEncode(#[from] rmp_serde::encode::Error),

    #[error("msgpack decode: {0}")]
    MsgpackDecode(#[from] rmp_serde::decode::Error),

    #[error("websocket: {0}")]
    WebSocket(String),

    #[error("vault: {0}")]
    Vault(String),

    #[error("size limit exceeded: {0}")]
    TooLarge(String),

    #[error("invalid utf8")]
    InvalidUtf8,

    #[error("{0}")]
    Other(String),
}

#[cfg(not(target_arch = "wasm32"))]
impl From<tokio_tungstenite::tungstenite::Error> for Error {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        Error::WebSocket(e.to_string())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
