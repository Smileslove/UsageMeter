use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::Deref;

macro_rules! string_identity {
    ($name:ident) => {
        #[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub(crate) struct $name(String);

        impl $name {
            pub(crate) fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl Deref for $name {
            type Target = str;

            fn deref(&self) -> &Self::Target {
                self.as_str()
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.as_str() == *other
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

string_identity!(ToolId);
string_identity!(ProviderId);
string_identity!(SourceId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_keep_wire_representation_compatible() {
        let tool = ToolId::from("claude_code");
        assert_eq!(tool.as_str(), "claude_code");
        assert_eq!(serde_json::to_string(&tool).unwrap(), r#""claude_code""#);
        assert_eq!(String::from(tool), "claude_code");
    }
}
