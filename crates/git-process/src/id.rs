//! Git object ids, validated once at construction so no reader re-checks the
//! hex digits, and [`IdError`], the refusal every validated identifier shares.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A malformed identifier, naming what was expected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {kind}: {value:?} ({expected})")]
pub struct IdError {
    kind: &'static str,
    value: String,
    expected: &'static str,
}

impl IdError {
    /// `value` refused as a `kind`, which must be `expected`. Only the first
    /// 80 characters of `value` are kept.
    #[must_use]
    pub fn new(kind: &'static str, value: &str, expected: &'static str) -> Self {
        Self {
            kind,
            value: value.chars().take(80).collect(),
            expected,
        }
    }
}

/// A Git object id: 40 lowercase hex digits (SHA-1) or 64 (SHA-256).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ObjectId(String);

impl ObjectId {
    /// # Errors
    /// Refuses anything but 40 or 64 lowercase hex digits.
    pub fn parse(text: &str) -> Result<Self, IdError> {
        if matches!(text.len(), 40 | 64)
            && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            Ok(Self(text.to_owned()))
        } else {
            Err(IdError::new(
                "git object id",
                text,
                "40 or 64 lowercase hex digits",
            ))
        }
    }

    /// The validated text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

impl TryFrom<String> for ObjectId {
    type Error = IdError;
    fn try_from(value: String) -> Result<Self, IdError> {
        Self::parse(&value)
    }
}

impl From<ObjectId> for String {
    fn from(id: ObjectId) -> Self {
        id.0
    }
}

impl std::str::FromStr for ObjectId {
    type Err = IdError;
    fn from_str(text: &str) -> Result<Self, IdError> {
        Self::parse(text)
    }
}

macro_rules! object_kind {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(ObjectId);

        impl $name {
            /// # Errors
            /// Refuses a malformed object id.
            pub fn parse(text: &str) -> Result<Self, IdError> {
                ObjectId::parse(text).map(Self)
            }

            /// The object id.
            #[must_use]
            pub fn object(&self) -> &ObjectId {
                &self.0
            }

            /// The hex text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                self.0.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

object_kind! {
    /// A commit.
    CommitId
}
object_kind! {
    /// A tree: what a commit's content is, and what survives a rewrite that
    /// keeps the content (a jj rebase, an amended message).
    TreeId
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_ids_accept_sha1_and_sha256_lengths_only() {
        assert!(ObjectId::parse(&"a".repeat(40)).is_ok());
        assert!(ObjectId::parse(&"0".repeat(64)).is_ok());
        assert!(ObjectId::parse(&"a".repeat(39)).is_err());
        assert!(ObjectId::parse(&"A".repeat(40)).is_err());
        assert!(CommitId::parse("main").is_err());
    }

    #[test]
    fn a_refusal_keeps_only_a_bounded_prefix_of_the_value() {
        let long = "z".repeat(500);
        let error = ObjectId::parse(&long).expect_err("not hex");
        assert!(error.to_string().contains(&"z".repeat(80)));
        assert!(!error.to_string().contains(&"z".repeat(81)));
    }
}
