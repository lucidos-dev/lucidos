use super::BoxError;
use serde::{Deserialize, Serialize};

const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A validated directory name; callers cannot construct one from unchecked text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Handle(String);

impl Handle {
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut bytes = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let millis = chrono::Utc::now().timestamp_millis() as u64;
        bytes[..6].copy_from_slice(&millis.to_be_bytes()[2..]);
        let mut value = u128::from_be_bytes(bytes);
        let mut encoded = [b'0'; 26];
        for digit in encoded.iter_mut().rev() {
            *digit = ALPHABET[(value & 31) as usize];
            value >>= 5;
        }
        Self(format!(
            "snap_{}",
            String::from_utf8(encoded.to_vec()).expect("ASCII alphabet")
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Handle {
    type Error = BoxError;
    fn try_from(value: String) -> Result<Self, BoxError> {
        let suffix = value
            .strip_prefix("snap_")
            .ok_or("invalid snapshot handle")?;
        if suffix.len() != 26
            || !suffix.bytes().all(|b| ALPHABET.contains(&b))
            || suffix.bytes().next().is_none_or(|b| b > b'7')
        {
            return Err("invalid snapshot handle".into());
        }
        Ok(Self(value))
    }
}

impl From<Handle> for String {
    fn from(handle: Handle) -> String {
        handle.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handles_reject_paths_and_invalid_ulids() {
        for value in [
            "",
            "..",
            "snap_../x",
            "/tmp/snap",
            "snap_81JBX6Q4T5Z8W2N9R3C7D1F0HK",
            "snap_01jbx6q4t5z8w2n9r3c7d1f0hk",
            "snap_01IBX6Q4T5Z8W2N9R3C7D1F0HK",
        ] {
            assert!(Handle::try_from(value.to_owned()).is_err(), "{value}");
            assert!(serde_json::from_value::<Handle>(serde_json::json!(value)).is_err());
        }
        let generated = Handle::generate();
        assert_eq!(
            Handle::try_from(generated.as_str().to_owned()).unwrap(),
            generated
        );
    }
}
