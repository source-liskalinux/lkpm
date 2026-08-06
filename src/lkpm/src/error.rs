use std::fmt;

#[derive(Debug)]
pub enum LkpmError {
    Network(String),
    PackageNotFound(String),
    PermissionDenied,
    Io(std::io::Error),
    Other(String),
}

impl fmt::Display for LkpmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(msg) => write!(f, "Network error: {}. Check your internet connection first with 'ping' before running lkpm!", msg),
            Self::PackageNotFound(msg) => write!(f, "Package not found: {}. Try running 'lkpm -r' to update repository metadata if the package exists in any configured repository!", msg),
            Self::PermissionDenied => write!(f, "Root permission required. Use 'sudo' for this operation!"),
            Self::Io(e) => write!(f, "{}", e),
            Self::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for LkpmError {}

impl From<std::io::Error> for LkpmError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<anyhow::Error> for LkpmError {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err.to_string())
    }
}
