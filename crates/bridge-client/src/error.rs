use crate::protocol::ReplyError;

/// Errors returned by [`crate::BridgeClient`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BridgeError {
    /// No reply within the configured timeout. The socket has been reset.
    #[error("bridge did not reply within {0} ms")]
    Timeout(u64),
    /// Socket could not connect, send or receive.
    #[error("bridge transport error: {0}")]
    Transport(String),
    /// Reply was not valid protocol JSON, or its `id` did not match the request.
    #[error("bridge protocol error: {0}")]
    Protocol(String),
    /// The EA answered `ok: false`.
    #[error("bridge error (retcode {retcode}): {message}")]
    Remote { message: String, retcode: i64 },
}

impl From<ReplyError> for BridgeError {
    fn from(e: ReplyError) -> Self {
        match e {
            ReplyError::Remote { message, retcode } => BridgeError::Remote { message, retcode },
            ReplyError::Decode(m) => BridgeError::Protocol(m),
        }
    }
}
