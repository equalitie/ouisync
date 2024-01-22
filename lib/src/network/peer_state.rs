use super::runtime_id::PublicRuntimeId;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::{
    de::{self, SeqAccess, Unexpected, Visitor},
    ser::SerializeTuple,
    Deserialize, Deserializer, Serialize, Serializer,
};
use std::fmt;

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub enum PeerState {
    Known,
    Connecting,
    Handshaking,
    Active(PublicRuntimeId),
}

impl Serialize for PeerState {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Known => PeerStateKind::Known.serialize(s),
            Self::Connecting => PeerStateKind::Connecting.serialize(s),
            Self::Handshaking => PeerStateKind::Handshaking.serialize(s),
            Self::Active(id) => {
                let mut t = s.serialize_tuple(2)?;
                t.serialize_element(&PeerStateKind::Active)?;
                t.serialize_element(id)?;
                t.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for PeerState {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PeerStateVisitor;

        impl<'de> Visitor<'de> for PeerStateVisitor {
            type Value = PeerState;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(
                    f,
                    "one of {}, {}, {} or a pair of {} and a byte array",
                    u8::from(PeerStateKind::Known),
                    u8::from(PeerStateKind::Connecting),
                    u8::from(PeerStateKind::Handshaking),
                    u8::from(PeerStateKind::Active),
                )
            }

            fn visit_u8<E>(self, v: u8) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match PeerStateKind::try_from(v) {
                    Ok(PeerStateKind::Known) => Ok(PeerState::Known),
                    Ok(PeerStateKind::Connecting) => Ok(PeerState::Connecting),
                    Ok(PeerStateKind::Handshaking) => Ok(PeerState::Handshaking),
                    Ok(PeerStateKind::Active) => {
                        Err(E::invalid_value(Unexpected::Unsigned(v.into()), &self))
                    }
                    Err(_) => Err(E::invalid_value(Unexpected::Unsigned(v.into()), &self)),
                }
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let v = v
                    .try_into()
                    .map_err(|_| E::invalid_value(Unexpected::Unsigned(v), &self))?;
                self.visit_u8(v)
            }

            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let v = v
                    .try_into()
                    .map_err(|_| E::invalid_value(Unexpected::Signed(v), &self))?;
                self.visit_u8(v)
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                match seq.next_element()? {
                    Some(PeerStateKind::Known) => Ok(PeerState::Known),
                    Some(PeerStateKind::Connecting) => Ok(PeerState::Connecting),
                    Some(PeerStateKind::Handshaking) => Ok(PeerState::Handshaking),
                    Some(PeerStateKind::Active) => {
                        if let Some(id) = seq.next_element()? {
                            Ok(PeerState::Active(id))
                        } else {
                            Err(<A::Error as de::Error>::invalid_length(1, &self))
                        }
                    }
                    None => Err(<A::Error as de::Error>::invalid_length(0, &self)),
                }
            }
        }

        // NOTE: This doesn't work for non self-describing formats like bincode. We currently don't
        // use bincode for this type so this is not a problem.
        d.deserialize_any(PeerStateVisitor)
    }
}

/// State of the peer connection.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, IntoPrimitive, TryFromPrimitive)]
#[repr(u8)]
#[serde(into = "u8", try_from = "u8")]
pub enum PeerStateKind {
    /// The peer is known (discovered or explicitly added by the user) but we haven't started
    /// establishing a connection to them yet.
    Known,
    /// A connection to the peer is being established.
    Connecting,
    /// The peer is connected but the protocol handshake is still in progress.
    Handshaking,
    /// The peer connection is active.
    Active,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::runtime_id::SecretRuntimeId;
    use serde_test::{assert_tokens, Token};

    #[test]
    fn serialize_deserialize_tokens() {
        let id = SecretRuntimeId::random().public();

        assert_tokens(&PeerState::Known, &[Token::U8(PeerStateKind::Known.into())]);
        assert_tokens(
            &PeerState::Connecting,
            &[Token::U8(PeerStateKind::Connecting.into())],
        );
        assert_tokens(
            &PeerState::Handshaking,
            &[Token::U8(PeerStateKind::Handshaking.into())],
        );
        assert_tokens(
            &PeerState::Active(id),
            &[
                Token::Tuple { len: 2 },
                Token::U8(PeerStateKind::Active.into()),
                Token::BorrowedBytes(Box::leak(id.as_ref().to_vec().into_boxed_slice())),
                Token::TupleEnd,
            ],
        );
    }

    #[test]
    fn serialize_deserialize_json() {
        let states = [
            PeerState::Known,
            PeerState::Connecting,
            PeerState::Handshaking,
            PeerState::Active(SecretRuntimeId::random().public()),
        ];

        for state in states {
            let s = serde_json::to_string(&state).unwrap();
            let d: PeerState = serde_json::from_str(&s).unwrap();
            assert_eq!(d, state);
        }
    }
}
