package org.equalitie.ouisync.lib

import org.msgpack.core.MessagePacker
import org.msgpack.core.MessageUnpacker

data class MonitorId(val name: String, val disambiguator: Long) {
    companion object {
        fun decode(u: MessageUnpacker): MonitorId {
            val raw = u.unpackString()

            // A string in the format "name:disambiguator".
            val colon = raw.lastIndexOf(':')
            val name = raw.substring(0..colon)
            val disambiguator = raw.substring(colon + 1).toLong()

            return MonitorId(name, disambiguator)
        }
    }

    fun encode(p: MessagePacker) = p.packString(toString())

    override fun toString() = "$name:$disambiguator"
}

class StateMonitorNode(val values: Map<String, String>, val children: List<MonitorId>) {
    companion object {
        fun decode(u: MessageUnpacker): StateMonitorNode {
            if (u.unpackArrayHeader() != 2) {
                throw DecodeError()
            }

            val values = decodeValues(u)
            val children = decodeChildren(u)

            return StateMonitorNode(values, children)
        }

        private fun decodeValues(u: MessageUnpacker): Map<String, String> {
            val n = u.unpackMapHeader()
            return buildMap {
                repeat(n) {
                    val k = u.unpackString()
                    val v = u.unpackString()
                    put(k, v)
                }
            }
        }

        private fun decodeChildren(u: MessageUnpacker): List<MonitorId> {
            val n = u.unpackArrayHeader()
            return buildList {
                repeat(n) {
                    add(MonitorId.decode(u))
                }
            }
        }
    }
}
