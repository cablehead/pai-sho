//! Invitations: what one daemon hands another so it can say yes.
//!
//! An invitation is self-contained. It carries the issuer's key, so the
//! accepter knows which machine to dial, and a one-time code, which admits it.
//! Both
//! halves always travel together, so they are one value.
//!
//! `accept` also takes a bare key, for the case where nothing secret can be
//! put in front of the accepter (see docs/adr/0003-host-attested-enrollment.md).
//! `Handle` is whichever of the two arrived.

use anyhow::{anyhow, Context, Result};
use iroh::EndpointId;
use std::fmt;

/// Separates the issuer's key from the one-time code. A key prints as base32
/// and a code as hex, so neither contains it.
const SEP: char = '.';

/// An invitation: who to dial, and the code that admits you.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Invite {
    pub key: EndpointId,
    pub code: String,
}

impl fmt::Display for Invite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}{}", self.key, SEP, self.code)
    }
}

/// What `accept` was given: an invitation, or a key on its own.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Handle {
    Invite(Invite),
    Key(EndpointId),
}

impl Handle {
    /// The key to dial, either way.
    pub fn key(&self) -> EndpointId {
        match self {
            Handle::Invite(i) => i.key,
            Handle::Key(k) => *k,
        }
    }

    /// The code to present, if there is one.
    pub fn code(&self) -> Option<String> {
        match self {
            Handle::Invite(i) => Some(i.code.clone()),
            Handle::Key(_) => None,
        }
    }
}

impl std::str::FromStr for Handle {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        match s.split_once(SEP) {
            Some((key, code)) => {
                if code.is_empty() {
                    return Err(anyhow!("invitation has no code"));
                }
                let key: EndpointId = key.parse().context("invitation has an invalid key")?;
                Ok(Handle::Invite(Invite {
                    key,
                    code: code.to_string(),
                }))
            }
            None => Ok(Handle::Key(s.parse().context("invalid key")?)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> EndpointId {
        let mut bytes = [0u8; 32];
        bytes[0] = n;
        iroh::SecretKey::from_bytes(&bytes).public()
    }

    #[test]
    fn an_invitation_round_trips() {
        let invite = Invite {
            key: key(1),
            code: "abc123".into(),
        };
        let parsed: Handle = invite.to_string().parse().unwrap();
        assert_eq!(parsed, Handle::Invite(invite));
    }

    #[test]
    fn a_bare_key_parses_as_a_key() {
        let parsed: Handle = key(1).to_string().parse().unwrap();
        assert_eq!(parsed, Handle::Key(key(1)));
    }

    #[test]
    fn an_invitation_and_a_key_agree_on_who_to_dial() {
        let k = key(1);
        let invite: Handle = Invite {
            key: k,
            code: "abc".into(),
        }
        .to_string()
        .parse()
        .unwrap();
        let bare: Handle = k.to_string().parse().unwrap();
        assert_eq!(invite.key(), bare.key());
        assert_eq!(invite.code(), Some("abc".to_string()));
        assert_eq!(bare.code(), None);
    }

    #[test]
    fn junk_is_rejected() {
        assert!("not-a-key".parse::<Handle>().is_err());
        assert!("".parse::<Handle>().is_err());
    }

    #[test]
    fn an_invitation_without_a_code_is_rejected() {
        assert!(format!("{}.", key(1)).parse::<Handle>().is_err());
    }

    #[test]
    fn an_invitation_with_a_bad_key_is_rejected() {
        assert!("nonsense.abc123".parse::<Handle>().is_err());
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        let s = format!("  {}  ", key(1));
        assert_eq!(s.parse::<Handle>().unwrap(), Handle::Key(key(1)));
    }
}
