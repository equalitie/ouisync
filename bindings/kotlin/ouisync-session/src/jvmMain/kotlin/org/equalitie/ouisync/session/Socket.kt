package org.equalitie.ouisync.session

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.net.StandardProtocolFamily
import java.net.UnixDomainSocketAddress
import java.nio.channels.SocketChannel

internal actual suspend fun connectSocket(addr: SocketAddress): Socket {
    val channel =
        when (addr) {
            is SocketAddress.Unix -> SocketChannel.open(StandardProtocolFamily.UNIX)
            is SocketAddress.Tcp -> SocketChannel.open()
        }

    val socketAddr =
        when (addr) {
            is SocketAddress.Unix -> UnixDomainSocketAddress.of(addr.path)
            is SocketAddress.Tcp -> addr.addr
        }

    withContext(Dispatchers.IO) { channel.connect(socketAddr) }

    return CommonSocket(channel)
}
