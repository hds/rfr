use std::{fmt, io, str::FromStr};

use serde::{de::Visitor, Deserialize, Serialize};

/// Represents the RFR format variant used to encode a file.
///
/// A format variant distinguishes different RFR file formats, either streaming or chunked.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatVariant {
    /// The streaming RFR variant. The string representation is `rfr-s`.
    RfrStreaming,
    /// The chunked RFR variant. The string representation is `rfr-c`.
    RfrChunked,
}

impl fmt::Display for FormatVariant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FormatVariant {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "rfr-s" => Some(Self::RfrStreaming),
            "rfr-c" => Some(Self::RfrChunked),
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::RfrStreaming => "rfr-s",
            Self::RfrChunked => "rfr-c",
        }
    }
}

/// Represents the variant and version of the RFR file format.
///
/// The format identifier is present at the beginning of all flight recording data.
///
/// The identifier is made up of a [variant] and a version. The version is specific to the variant
/// (no compatibility between variants of the same version is implied).
///
/// # Semantic versioning
///
/// The version is specified following [semantic versioning]. It uses the [same treatment of leading zeros
/// as Cargo](https://doc.rust-lang.org/cargo/reference/semver.html), which is to say that changes
/// in the left-most non-zero part is considered a major (and hence potentially breaking) change.
///
/// For example, `0.1.2`to `0.2.0` is considered a major change.
///
/// # String representation
///
/// The format identifier has a string representation which follows the format
/// `<variant>/<major>.<minor>.<patch>`, where each part is the [`Display`] representation of that
/// field.
///
/// As an example, `rfr-s/0.1.2` represents the RFR streaming file format at version `0.1.2` (major
/// `0`, minor `1`, patch `1`).
///
/// The string representation can be constructed via the [`Display`] trait. That representation can
/// be parsed into a format identifier object by its [`FromStr::from_str`] implementation, see
/// [`FormatIdentifier::from_str`] for details and the `Err` values that can be returned.
///
/// [`Display`]: trait@std::fmt::Display
/// [variant]: enum@FormatVariant
/// [semantic versioning]: https://semver.org/
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatIdentifier {
    /// The RFR format variant.
    pub variant: FormatVariant,
    /// The major part of the version.
    pub major: u32,
    /// The minor part of the version.
    pub minor: u32,
    /// The patch part of the version.
    pub patch: u32,
}

impl fmt::Display for FormatIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{variant}/{major}.{minor}.{patch}",
            variant = self.variant,
            major = self.major,
            minor = self.minor,
            patch = self.patch,
        )
    }
}

impl FromStr for FormatIdentifier {
    /// The error returned when a string representation is invalid and cannot be parsed into a
    /// format identifier object.
    type Err = ParseFormatVersionError;

    /// Attempt to parse a string containing a format identifier string representation.
    ///
    /// See the [String representation] section of the struct documentation for details.
    ///
    /// [String representation]: struct@FormatIdentifier#string-representation
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts = s.split('/').collect::<Vec<_>>();
        if parts.len() != 2 {
            return Err(ParseFormatVersionError::IncorrectParts);
        }

        let raw_variant = parts[0];
        let variant = FormatVariant::from_str(raw_variant)
            .ok_or_else(|| ParseFormatVersionError::UnknownVariant(parts[0].into()))?;

        let version = parts[1];
        let ver_parts = version.split('.').collect::<Vec<_>>();
        if ver_parts.len() != 3 {
            return Err(ParseFormatVersionError::InvalidVersion(version.into()));
        }

        let invalid_version = |_err| ParseFormatVersionError::InvalidVersion(version.into());
        let major = ver_parts[0].parse().map_err(invalid_version)?;
        let minor = ver_parts[1].parse().map_err(invalid_version)?;
        let patch = ver_parts[2].parse().map_err(invalid_version)?;

        Ok(Self {
            variant,
            major,
            minor,
            patch,
        })
    }
}

/// Error attempting to parse a [FormatIdentifier] from its string representation.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseFormatVersionError {
    /// The string representation had too many or too few parts when split by the `/` character.
    /// There should be exactly one `/` character in the string representation.
    IncorrectParts,
    /// The variant (everything before the `/`) is unknown. See [FormatVariant] for all valid
    /// values. The included `String` is the variant that could not be identified.
    UnknownVariant(String),
    /// The version part (everything after the `/`) is invalid. A valid version should be
    /// `<major>.<minor>.<patch>`, where each part is a decimal integer. For example `1.2.3`. The
    /// included `String` is the version that could not be parsed.
    InvalidVersion(String),
}

impl fmt::Display for ParseFormatVersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IncorrectParts => write!(
                f,
                "expected '<variant>/<major>.<minor>.<patch>', \
                but didn't find exactly 2 parts separated by a '/' character"
            ),
            Self::UnknownVariant(variant) => write!(f, "variant is not recognised: {variant}"),
            Self::InvalidVersion(version) => write!(
                f,
                "expected version to be '<major>.<minor>.<patch>', but instead found '{version}'"
            ),
        }
    }
}

impl FormatIdentifier {
    pub fn try_from_io(reader: impl io::Read) -> Result<Self, ReadFormatIdentifierError> {
        let mut reader = reader;
        let mut buffer = vec![0_u8; 24];

        match postcard::from_io((&mut reader, buffer.as_mut_slice())) {
            Ok((raw_value, _)) => FormatIdentifier::from_str(raw_value)
                .map_err(ReadFormatIdentifierError::FormatIdentifierInvalid),
            Err(postcard::Error::DeserializeUnexpectedEnd) => {
                Err(ReadFormatIdentifierError::FormatIdentifierTooLong)
            }
            Err(postcard_error) => Err(ReadFormatIdentifierError::PostcardReadFailed(
                postcard_error,
            )),
        }
    }
}

#[derive(Debug)]
pub enum ReadFormatIdentifierError {
    PostcardReadFailed(postcard::Error),
    FormatIdentifierTooLong,
    FormatIdentifierInvalid(ParseFormatVersionError),
}

impl FormatIdentifier {
    /// Returns whether or not the receiver can read data written by `version`.
    ///
    /// This function an exact match on the variant and semantic versioning rules to determine the
    /// returned value. It doesn't check that file format specifications for those particular
    /// versions actually exist.
    ///
    /// See the [struct documentation] for how semantic versioning is used.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rfr::{FormatIdentifier, FormatVariant};
    /// let version = FormatIdentifier {
    ///     variant: FormatVariant::RfrStreaming,
    ///     major: 0,
    ///     minor: 1,
    ///     patch: 4,
    /// };
    ///
    /// let other = FormatIdentifier {
    ///     variant: FormatVariant::RfrStreaming,
    ///     major: 0,
    ///     minor: 1,
    ///     patch: 2,
    /// };
    ///
    /// assert!(version.can_read_version(&other));
    /// ```
    ///
    /// [struct documentation]: struct@FormatIdentifier
    pub fn can_read_version(&self, version: &FormatIdentifier) -> bool {
        let current = self;

        // Completely different format
        if current.variant != version.variant {
            return false;
        }

        // Different major version
        if current.major != version.major {
            return false;
        }

        // Pre 1.0.0
        if current.major == 0 {
            // Different minor in pre-1.0
            if current.minor != version.minor {
                return false;
            }

            // Pre 0.1.0
            if current.minor == 0 {
                // Different patch in pre-0.1.0
                if current.patch != version.patch {
                    return false;
                }
            }

            if current.patch >= version.patch {
                return true;
            }
        }

        if current.minor >= version.minor {
            return true;
        }

        false
    }
}

impl Serialize for FormatIdentifier {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let stringly = format!("{self}");
        serializer.serialize_str(&stringly)
    }
}

struct FormatIdentifierVisitor {}

impl<'de> Visitor<'de> for FormatIdentifierVisitor {
    type Value = FormatIdentifier;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a string representation of a format identifier, e.g. 'rfr-c/0.1.2'")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        FormatIdentifier::from_str(v).map_err(serde::de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for FormatIdentifier {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(FormatIdentifierVisitor {})
    }
}
