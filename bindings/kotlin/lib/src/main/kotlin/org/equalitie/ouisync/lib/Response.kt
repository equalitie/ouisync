package org.equalitie.ouisync.lib

// import org.msgpack.core.MessageUnpacker
// import org.msgpack.value.ValueType

// /**
//  * Information about a peer.
//  *
//  * @property addr      remote address of the peer in the "PROTOCOL/IP:PORT" format.
//  * @property source    how was the peer discovered.
//  * @property state     state of the peer connection.
//  * @property runtimeId [runtime id][Session.thisRuntimeId] of the peer if [active][PeerStateKind.ACTIVE], otherwise null.
//  */
// // TODO: stats
// data class PeerInfo(
//     val addr: String,
//     val source: PeerSource,
//     val state: PeerState,
// )

// sealed class PeerState {
//     object Known : PeerState()
//     object Connecting : PeerState()
//     object Handshaking : PeerState()
//     class Active(val runtimeId: String) : PeerState()
// }

// data class Progress(val value: Long, val total: Long)

// internal sealed interface Response {
//     companion object {
//         fun unpack(unpacker: MessageUnpacker): Response {
//             val type = unpacker.getNextFormat().getValueType()

//             if (type != ValueType.MAP) {
//                 throw Error.InvalidData("invalid response: expected MAP, was $type")
//             }

//             if (unpacker.unpackMapHeader() < 1) {
//                 throw Error.InvalidData("invalid response: empty map")
//             }

//             val kind = unpacker.unpackString()

//             when (kind) {
//                 "success" -> return unpackSuccess(unpacker)
//                 "failure" -> return unpackFailure(unpacker)
//                 else -> throw Error.InvalidData("invalid response type: expected 'success' or 'failuire', was '$kind'")
//             }
//         }

//         private fun unpackSuccess(unpacker: MessageUnpacker): Response {
//             val type = unpacker.getNextFormat().getValueType()

//             when (type) {
//                 ValueType.STRING -> {
//                     val value = unpacker.unpackString()
//                     when (value) {
//                         "none", "repository_event", "state_monitor_event" -> return Success(Unit)
//                         else -> throw Error.InvalidData("invalid response payload: '$value'")
//                     }
//                 }
//                 ValueType.MAP -> {
//                     val size = unpacker.unpackMapHeader()
//                     if (size < 1) {
//                         throw Error.InvalidData("invalid response payload: empty map")
//                     }

//                     val name = unpacker.unpackString()
//                     val value = when (name) {
//                         "access_mode" -> AccessMode.decode(unpacker.unpackByte())
//                         "bool" -> unpacker.unpackBoolean()
//                         "bytes" -> unpacker.unpackByteArray()
//                         "directory" -> unpacker.unpackDirectory()
//                         // Duration(Duration),
//                         "entry_type" -> EntryType.decode(unpacker.unpackByte())
//                         "file", "repository", "u64" -> unpacker.unpackLong()
//                         "network_event" -> NetworkEvent.decode(unpacker.unpackByte())
//                         // NetworkStats(Stats),
//                         "peer_addrs" -> unpacker.unpackStringList()
//                         "peer_info" -> unpacker.unpackPeerInfoList()
//                         "progress" -> unpacker.unpackProgress()
//                         // QuotaInfo(QuotaInfo),
//                         "repositories" -> unpacker.unpackLongMap()
//                         // SocketAddr(#[serde(with = "helpers::str")] SocketAddr),
//                         // StateMonitor(StateMonitor),
//                         // StorageSize(StorageSize),
//                         "path", "share_token", "string" -> unpacker.unpackString()
//                         // U32(u32),
//                         else -> throw Error.InvalidData("invalid response payload: '$name'")
//                     }

//                     return Success(value)
//                 }
//                 else -> throw Error.InvalidData("invalid response payload: expected STRING or MAP, was $type")
//             }
//         }

//         private fun unpackFailure(unpacker: MessageUnpacker): Failure {
//             val type = unpacker.getNextFormat().getValueType()
//             if (type != ValueType.ARRAY) {
//                 throw Error.InvalidData("invalid response payload: expected ARRAY, was $type")
//             }

//             val size = unpacker.unpackArrayHeader()
//             if (size < 2) {
//                 throw Error.InvalidData("invalid response array size: expected 2, was $size")
//             }

//             val code = ErrorCode.decode(unpacker.unpackInt().toShort())
//             val message = unpacker.unpackString()

//             val sources = if (size > 2) {
//                 unpacker.unpackStringList()
//             } else {
//                 emptyList()
//             }

//             return Failure(Error.dispatch(code, message, sources))
//         }
//     }
// }

// internal class Success(val value: Any) : Response
// internal class Failure(val error: Error) : Response

// private fun MessageUnpacker.unpackByteArray(): ByteArray {
//     val size = unpackBinaryHeader()
//     return readPayload(size)
// }

// private fun MessageUnpacker.unpackStringList(): List<String> {
//     val size = unpackArrayHeader()
//     val list = ArrayList<String>(size)

//     repeat(size) {
//         list.add(unpackString())
//     }

//     return list
// }

// private fun MessageUnpacker.unpackPeerInfoList(): List<PeerInfo> {
//     val size = unpackArrayHeader()
//     val list = ArrayList<PeerInfo>(size)

//     repeat(size) {
//         list.add(unpackPeerInfo())
//     }

//     return list
// }

// private fun MessageUnpacker.unpackPeerInfo(): PeerInfo {
//     val size = unpackArrayHeader()
//     if (size < 3) {
//         throw Error.InvalidData("invalid PeerInfo: too few elements")
//     }

//     val addr = unpackString()
//     val source = PeerSource.decode(unpackByte())
//     val state = unpackPeerState()

//     // TODO: stats

//     return PeerInfo(addr, source, state)
// }

// private fun MessageUnpacker.unpackPeerState(): PeerState {
//     val type = getNextFormat().getValueType()

//     when (type) {
//         ValueType.INTEGER -> {
//             return when (PeerStateKind.decode(unpackByte())) {
//                 PeerStateKind.KNOWN -> PeerState.Known
//                 PeerStateKind.CONNECTING -> PeerState.Connecting
//                 PeerStateKind.HANDSHAKING -> PeerState.Handshaking
//                 PeerStateKind.ACTIVE -> throw Error.InvalidData("invalid PeerState: missing runtime id")
//             }
//         }
//         ValueType.ARRAY -> {
//             if (unpackArrayHeader() < 2) {
//                 throw Error.InvalidData("invalid PeerState: too few elements")
//             }

//             val kind = PeerStateKind.decode(unpackByte())

//             @OptIn(kotlin.ExperimentalStdlibApi::class)
//             val runtimeId = unpackByteArray().toHexString()

//             return when (kind) {
//                 PeerStateKind.KNOWN -> PeerState.Known
//                 PeerStateKind.CONNECTING -> PeerState.Connecting
//                 PeerStateKind.HANDSHAKING -> PeerState.Handshaking
//                 PeerStateKind.ACTIVE -> PeerState.Active(runtimeId)
//             }
//         }
//         else -> throw Error.InvalidData("invalid PeerState type: expected INTEGER or ARRAY, was $type")
//     }
// }

// private fun MessageUnpacker.unpackProgress(): Progress {
//     if (unpackArrayHeader() < 2) {
//         throw Error.InvalidData("invalid Progress: too few elements")
//     }

//     val value = unpackLong()
//     val total = unpackLong()

//     return Progress(value, total)
// }

// private fun MessageUnpacker.unpackDirectory(): Directory {
//     val count = unpackArrayHeader()
//     val entries = 0.rangeUntil(count).map { unpackDirectoryEntry() }

//     return Directory(entries)
// }

// internal fun MessageUnpacker.unpackDirectoryEntry(): DirectoryEntry {
//     if (unpackArrayHeader() < 2) {
//         throw Error.InvalidData("invalid DirectoryEntry: too few elements")
//     }

//     val name = unpackString()
//     val entryType = EntryType.decode(unpackByte())

//     return DirectoryEntry(name, entryType)
// }

// internal fun MessageUnpacker.unpackLongMap(): Map<String, Long> {
//     val count = unpackMapHeader()

//     return 0.rangeUntil(count).associate {
//         val key = unpackString()
//         val value = unpackLong()

//         Pair(key, value)
//     }
// }
