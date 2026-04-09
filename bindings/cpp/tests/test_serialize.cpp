#define BOOST_TEST_MODULE Serialize
#include <boost/test/included/unit_test.hpp>

#include <stdexcept>
#include <type_traits>
#include <ouisync.hpp>
#include <ouisync/serialize.hpp>

// Decode HEX digit into byte
char decode_hex_digit(char input) {
    switch (input) {
        case '0': return 0;
        case '1': return 1;
        case '2': return 2;
        case '3': return 3;
        case '4': return 4;
        case '5': return 5;
        case '6': return 6;
        case '7': return 7;
        case '8': return 8;
        case '9': return 9;
        case 'a':
        case 'A': return 10;
        case 'b':
        case 'B': return 11;
        case 'c':
        case 'C': return 12;
        case 'd':
        case 'D': return 13;
        case 'e':
        case 'E': return 14;
        case 'f':
        case 'F': return 15;
    }

    throw std::invalid_argument(std::string("invalid hex digit: '") + input + "'");
}

template<typename T> concept Byte = std::is_same_v<T, char> || std::is_same_v<T, unsigned char>;

// Decode hex string into vector of bytes
template<Byte T>
std::vector<T> decode_hex(const std::string& input) {
    std::vector<T> out;
    out.reserve(input.size() / 2);

    for (std::size_t i = 0; i < input.size(); i += 2) {
        out.push_back(16 * decode_hex_digit(input[i]) + decode_hex_digit(input[i + 1]));
    }

    return out;
}

BOOST_AUTO_TEST_CASE(deserialize_peer_infos) {
    // hex-encoding of msgpack serialization of this data:
    //
    //     ResponseResult::Success(
    //         Response::PeerInfos(vec![
    //             PeerInfo {
    //                 addr: PeerAddr::Quic((Ipv4Addr::LOCALHOST, 1234).into()),
    //                 source: PeerSource::Listener,
    //                 state: PeerState::Connecting,
    //                 stats: Stats {
    //                     bytes_tx: 0,
    //                     bytes_rx: 0,
    //                     throughput_tx: 0,
    //                     throughput_rx: 0,
    //                 },
    //             },
    //             PeerInfo {
    //                 addr: PeerAddr::Quic((Ipv4Addr::LOCALHOST, 2468).into()),
    //                 source: PeerSource::UserProvided,
    //                 state: PeerState::Active {
    //                     id: runtime_id,
    //                     since: SystemTime::UNIX_EPOCH + Duration::from_secs(10),
    //                 },
    //                 stats: Stats {
    //                     bytes_tx: 0,
    //                     bytes_rx: 0,
    //                     throughput_tx: 0,
    //                     throughput_rx: 0,
    //                 },
    //             },
    //         ])
    //     )
    const std::string runtime_id_hex = "ee1aa49a4459dfe813a3cf6eb882041230c7b2558469de81f87c9bf23bf10a03";
    const std::string encoded_message_hex =
        "81a75375636365737381a950656572496e666f739294b3717569632f3132372e302e302e313a3132333401aa43"
        "6f6e6e656374696e67940000000094b3717569632f3132372e302e302e313a323436380081a641637469766592"
        "c420ee1aa49a4459dfe813a3cf6eb882041230c7b2558469de81f87c9bf23bf10a03cd27109400000000";

    auto encoded_message = decode_hex<char>(encoded_message_hex);
    auto result = ouisync::deserialize(encoded_message);
    auto response = std::get<ouisync::Response>(result.value);
    auto peer_infos = response.get<ouisync::Response::PeerInfos>();

    BOOST_REQUIRE_EQUAL(peer_infos.value.size(), 2);

    BOOST_REQUIRE_EQUAL(peer_infos.value[0].addr, "quic/127.0.0.1:1234");
    BOOST_REQUIRE_EQUAL(peer_infos.value[0].source, ouisync::PeerSource::listener);
    BOOST_REQUIRE(peer_infos.value[0].state.get_if<ouisync::PeerState::Connecting>());

    ouisync::Stats expected_stats { 0, 0, 0, 0 };
    BOOST_REQUIRE(peer_infos.value[0].stats == expected_stats);

    BOOST_REQUIRE_EQUAL(peer_infos.value[1].addr, "quic/127.0.0.1:2468");
    BOOST_REQUIRE_EQUAL(peer_infos.value[1].source, ouisync::PeerSource::user_provided);

    auto state = peer_infos.value[1].state.get<ouisync::PeerState::Active>();
    BOOST_REQUIRE(state.id.value == decode_hex<unsigned char>(runtime_id_hex));
    BOOST_REQUIRE_EQUAL(state.since, std::chrono::time_point<std::chrono::system_clock>(std::chrono::seconds(10)));

    BOOST_REQUIRE(peer_infos.value[1].stats == expected_stats);
}
