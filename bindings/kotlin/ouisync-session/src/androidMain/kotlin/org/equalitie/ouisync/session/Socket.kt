package org.equalitie.ouisync.session

import android.net.LocalSocket
import android.net.LocalSocketAddress
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.nio.ByteBuffer
import java.nio.channels.Channels
import java.nio.channels.SocketChannel

internal class AndroidUnixSocket(private val socket: LocalSocket) : Socket() {
    companion object {
        suspend fun connect(path: String): AndroidUnixSocket {
            val socket = LocalSocket()

            withContext(Dispatchers.IO) {
                socket.connect(LocalSocketAddress(path, LocalSocketAddress.Namespace.FILESYSTEM))
            }

            return AndroidUnixSocket(socket)
        }
    }

    private val reader = Channels.newChannel(socket.inputStream)
    private val writer = Channels.newChannel(socket.outputStream)

    override suspend fun read(buffer: ByteBuffer) = withContext(Dispatchers.IO) { reader.read(buffer) }

    override suspend fun write(buffer: ByteBuffer) = withContext(Dispatchers.IO) { writer.write(buffer) }

    override fun close() {
        reader.close()
        writer.close()
        socket.close()
    }
}

internal actual suspend fun connectSocket(addr: SocketAddress): Socket = when (addr) {
    is SocketAddress.Unix -> AndroidUnixSocket.connect(addr.path)
    is SocketAddress.Tcp -> CommonSocket(SocketChannel.open()).apply { connect(addr.addr) }
}
