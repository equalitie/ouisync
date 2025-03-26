use std::time::SystemTime;

use ouisync_macros::api;
use serde::{Deserialize, Serialize};

use super::runtime_id::PublicRuntimeId;

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize, Deserialize)]
#[api]
pub enum PeerState {
    Known,
    Connecting,
    Handshaking,
    Active {
        id: PublicRuntimeId,
        #[serde(with = "system_time")]
        since: SystemTime,
    },
}

mod system_time {
    use serde::{ser, Deserialize, Deserializer, Serialize, Serializer};

    use crate::time::{from_millis_since_epoch, to_millis_since_epoch};
    use std::time::SystemTime;

    pub(super) fn serialize<S: Serializer>(time: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        to_millis_since_epoch(*time)
            .map_err(ser::Error::custom)?
            .serialize(s)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        Ok(from_millis_since_epoch(u64::deserialize(d)?))
    }
}

#[cfg(test)]
mod tests {
    use crate::time::{from_millis_since_epoch, to_millis_since_epoch};

    use super::*;
    use crate::network::runtime_id::SecretRuntimeId;
    use serde_json::json;

    #[test]
    fn serialize_deserialize_json() {
        let id = SecretRuntimeId::random().public();
        let now = SystemTime::now();

        let test_vectors = [
            (PeerState::Known, json!("Known")),
            (PeerState::Connecting, json!("Connecting")),
            (PeerState::Handshaking, json!("Handshaking")),
            (
                PeerState::Active {
                    id,
                    // The timestamp is serialized as the number of milliseconds since the epoch so we
                    // need to round it to whole milliseconds otherwise the deserialized value would
                    // not be exactly equal to the original one.
                    since: round_to_millis(now),
                },
                json!({
                    "Active": {
                        "id": id.as_ref(),
                        "since": to_millis_since_epoch(now).unwrap(),
                    }
                }),
            ),
        ];

        for (input, expected) in test_vectors {
            let s = serde_json::to_string(&input).unwrap();
            let v: serde_json::Value = serde_json::from_str(&s).unwrap();

            assert_eq!(v, expected);

            let d: PeerState = serde_json::from_str(&s).unwrap();
            assert_eq!(d, input);
        }
    }

    #[test]
    fn serialize_deserialize_message_pack() {
        let id = SecretRuntimeId::random().public();
        let now = SystemTime::now();

        let test_vectors = [
            (PeerState::Known, {
                let mut out = Vec::new();
                rmp::encode::write_str(&mut out, "Known").unwrap();
                out
            }),
            (PeerState::Connecting, {
                let mut out = Vec::new();
                rmp::encode::write_str(&mut out, "Connecting").unwrap();
                out
            }),
            (PeerState::Handshaking, {
                let mut out = Vec::new();
                rmp::encode::write_str(&mut out, "Handshaking").unwrap();
                out
            }),
            (
                PeerState::Active {
                    id,
                    since: round_to_millis(now),
                },
                {
                    let mut out = Vec::new();
                    rmp::encode::write_map_len(&mut out, 1).unwrap();
                    rmp::encode::write_str(&mut out, "Active").unwrap();
                    rmp::encode::write_array_len(&mut out, 2).unwrap();
                    rmp::encode::write_bin(&mut out, id.as_ref()).unwrap();
                    rmp::encode::write_u64(&mut out, to_millis_since_epoch(now).unwrap()).unwrap();
                    out
                },
            ),
        ];

        for (input, expected) in test_vectors {
            let s = rmp_serde::to_vec(&input).unwrap();
            assert_eq!(s, expected);

            let d: PeerState = rmp_serde::from_slice(&s).unwrap();
            assert_eq!(d, input);
        }
    }

    fn round_to_millis(time: SystemTime) -> SystemTime {
        from_millis_since_epoch(to_millis_since_epoch(time).unwrap())
    }
}
