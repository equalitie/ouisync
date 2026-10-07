package org.equalitie.ouisync.session

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.Closeable
import java.net.InetSocketAddress
import java.nio.ByteBuffer
import java.nio.channels.SocketChannel

/** Address of a socket to connect to */
internal sealed interface SocketAddress {
    /** TCP socket address (ip address + port) */
    data class Tcp(val addr: InetSocketAddress) : SocketAddress

    /** Unix domain socket address (path in the filesystem) */
    data class Unix(val path: String) : SocketAddress
}

internal abstract class Socket : Closeable {
    companion object {
        suspend fun connect(addr: SocketAddress): Socket = connectSocket(addr)
    }

    abstract suspend fun read(buffer: ByteBuffer): Int

    abstract suspend fun write(buffer: ByteBuffer): Int
}

// Used for TCP on both jvm and android and for UNIX on jvm.
internal class CommonSocket(private val channel: SocketChannel) : Socket() {
    suspend fun connect(addr: java.net.SocketAddress) {
        withContext(Dispatchers.IO) { channel.connect(addr) }
    }

    override suspend fun read(buffer: ByteBuffer) = withContext(Dispatchers.IO) { channel.read(buffer) }

    override suspend fun write(buffer: ByteBuffer) = withContext(Dispatchers.IO) { channel.write(buffer) }

    override fun close() {
        channel.close()
    }
}

internal expect suspend fun connectSocket(addr: SocketAddress): Socket
