package org.equalitie.ouisync.session

import android.net.LocalSocket
import android.net.LocalSocketAddress
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.channels.Channels
import java.nio.channels.SocketChannel

// All operations are cancellable. Cancelling an operation closes the socket.
internal class AndroidUnixSocket : Socket() {
    private val socket = LocalSocket()
    private val reader = Channels.newChannel(socket.inputStream)
    private val writer = Channels.newChannel(socket.outputStream)

    suspend fun connect(path: String) {
        runCancellable {
            socket.connect(LocalSocketAddress(path, LocalSocketAddress.Namespace.FILESYSTEM))
        }
    }

    override suspend fun read(buffer: ByteBuffer) = runCancellable { reader.read(buffer) }

    override suspend fun write(buffer: ByteBuffer) = runCancellable { writer.write(buffer) }

    override fun close() {
        // Closing the `LocalSocket` doesn't unblock threads currently blocked in read or write on
        // it, but shutting it down does.
        try {
            socket.shutdownInput()
        } catch (_: IOException) {
            // Not connected or already closed.
        }

        try {
            socket.shutdownOutput()
        } catch (_: IOException) {
            // Not connected or already closed.
        }

        reader.close()
        writer.close()
        socket.close()
    }
}

internal actual suspend fun connectSocket(addr: SocketAddress): Socket = when (addr) {
    is SocketAddress.Unix -> AndroidUnixSocket().apply { connect(addr.path) }
    is SocketAddress.Tcp -> CommonSocket(SocketChannel.open()).apply { connect(addr.addr) }
}
