use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    UnknownKey(String),
    Deserialize { key: String, message: String },
    Serialize { key: String, message: String },
}

impl KeyError {
    pub fn unknown(key: impl Into<String>) -> Self {
        Self::UnknownKey(key.into())
    }

    pub fn deserialize(key: impl Into<String>, error: impl Display) -> Self {
        Self::Deserialize {
            key: key.into(),
            message: error.to_string(),
        }
    }

    pub fn serialize(key: impl Into<String>, error: impl Display) -> Self {
        Self::Serialize {
            key: key.into(),
            message: error.to_string(),
        }
    }
}

impl Display for KeyError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownKey(key) => write!(f, "unknown map key `{key}`"),
            Self::Deserialize { key, message } => {
                write!(f, "failed to deserialize map key `{key}`: {message}")
            }
            Self::Serialize { key, message } => {
                write!(f, "failed to serialize map key `{key}`: {message}")
            }
        }
    }
}

impl Error for KeyError {}
