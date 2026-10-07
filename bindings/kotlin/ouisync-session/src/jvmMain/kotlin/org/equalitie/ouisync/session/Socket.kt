package org.equalitie.ouisync.session

import java.net.StandardProtocolFamily
import java.net.UnixDomainSocketAddress
import java.nio.channels.SocketChannel

internal actual suspend fun connectSocket(addr: SocketAddress): Socket = when (addr) {
    is SocketAddress.Unix -> {
        CommonSocket(SocketChannel.open(StandardProtocolFamily.UNIX)).apply {
            connect(UnixDomainSocketAddress.of(addr.path))
        }
    }
    is SocketAddress.Tcp -> {
        CommonSocket(SocketChannel.open()).apply { connect(addr.addr) }
    }
}
