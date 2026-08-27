use std::error::Error;
use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalarError {
    rule: String,
}

impl ScalarError {
    fn new(rule: impl Into<String>) -> Self {
        Self { rule: rule.into() }
    }
}

impl Display for ScalarError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.rule)
    }
}

impl Error for ScalarError {}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BoundedText<const MIN: usize, const MAX: usize>(String);

impl<const MIN: usize, const MAX: usize> BoundedText<MIN, MAX> {
    pub fn new(value: String) -> Result<Self, ScalarError> {
        let length = value.chars().count();
        if (MIN..=MAX).contains(&length) {
            Ok(Self(value))
        } else {
            Err(ScalarError::new(format!(
                "text length must be between {MIN} and {MAX} characters"
            )))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

/// A nullable string whose key must be present on the wire.
///
/// `Option<String>` alone treats an absent field like JSON `null`. This
/// wrapper preserves the existing protocol distinction without relying on a
/// custom field attribute that contract generators cannot understand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct RequiredNullableString(Option<String>);

impl RequiredNullableString {
    #[must_use]
    pub const fn new(value: Option<String>) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn as_deref(&self) -> Option<&str> {
        self.0.as_deref()
    }

    #[must_use]
    pub fn into_inner(self) -> Option<String> {
        self.0
    }
}

impl<'de> Deserialize<'de> for RequiredNullableString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RequiredNullableStringVisitor;

        impl de::Visitor<'_> for RequiredNullableStringVisitor {
            type Value = RequiredNullableString;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
                formatter.write_str("a string or null")
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(RequiredNullableString(None))
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(RequiredNullableString(None))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(RequiredNullableString(Some(value.to_owned())))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(RequiredNullableString(Some(value)))
            }
        }

        // `deserialize_any` is intentional: JSON null reaches `visit_unit`,
        // while Serde's missing-field deserializer errors before invoking the
        // visitor. Delegating to `Option` would collapse missing and null.
        deserializer.deserialize_any(RequiredNullableStringVisitor)
    }
}

impl<const MAX: usize> Default for BoundedText<0, MAX> {
    fn default() -> Self {
        Self(String::new())
    }
}

impl<const MIN: usize, const MAX: usize> Serialize for BoundedText<MIN, MAX> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de, const MIN: usize, const MAX: usize> Deserialize<'de> for BoundedText<MIN, MAX> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

macro_rules! bounded_integer {
    ($name:ident, $min:expr, $max:expr, $default:expr) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(i64);

        impl $name {
            pub fn new(value: i64) -> Result<Self, ScalarError> {
                if ($min..=$max).contains(&value) {
                    Ok(Self(value))
                } else {
                    Err(ScalarError::new(format!(
                        "value must be between {} and {}",
                        $min, $max
                    )))
                }
            }

            #[must_use]
            pub const fn get(self) -> i64 {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self($default)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_i64(self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = i64::deserialize(deserializer)?;
                Self::new(value).map_err(de::Error::custom)
            }
        }
    };
}

bounded_integer!(NonNegativeI64, 0, i64::MAX, 0);
bounded_integer!(ProtocolVersion, 1, i64::MAX, 1);
bounded_integer!(CrossfadeMillis, 0, 30_000, 0);
bounded_integer!(FadeMillis, 0, 10_000, 0);

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct UnitInterval(f64);

impl UnitInterval {
    pub fn new(value: f64) -> Result<Self, ScalarError> {
        if value.is_finite() && (0.0..=1.0).contains(&value) {
            Ok(Self(value))
        } else {
            Err(ScalarError::new("value must be finite and between 0 and 1"))
        }
    }

    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl Default for UnitInterval {
    fn default() -> Self {
        Self(1.0)
    }
}

impl Serialize for UnitInterval {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_f64(self.0)
    }
}

impl<'de> Deserialize<'de> for UnitInterval {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = f64::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct LoopIntervalSeconds(f64);

impl LoopIntervalSeconds {
    pub fn new(value: f64) -> Result<Self, ScalarError> {
        if value.is_finite() && (1.0..=3600.0).contains(&value) {
            Ok(Self(value))
        } else {
            Err(ScalarError::new(
                "value must be finite and between 1 and 3600 seconds",
            ))
        }
    }

    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl Serialize for LoopIntervalSeconds {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_f64(self.0)
    }
}

impl<'de> Deserialize<'de> for LoopIntervalSeconds {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = f64::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}
