package org.equalitie.ouisync.session

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.ensureActive
import java.io.Closeable
import java.io.IOException
import java.net.InetSocketAddress
import java.nio.ByteBuffer
import java.nio.channels.SocketChannel
import java.util.concurrent.atomic.AtomicInteger

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

        private const val NOT_STARTED = 0
        private const val RUNNING = 1
        private const val CANCELLED = 2
    }

    abstract suspend fun read(buffer: ByteBuffer): Int

    abstract suspend fun write(buffer: ByteBuffer): Int

    /**
     * Runs the blocking [block] on the IO dispatcher. If the calling coroutine is cancelled while
     * [block] is running, closes this socket to unblock it, then waits for [block] to finish and
     * throws [CancellationException]. If cancelled before [block] started, the socket is not
     * closed.
     */
    protected suspend fun <T> runCancellable(block: () -> T): T = coroutineScope {
        // Whichever side (the block or the cancellation handler) transitions out of
        // `NOT_STARTED` first decides whether `block` runs and therefore whether closing is
        // needed.
        val state = AtomicInteger(NOT_STARTED)

        val result = async(Dispatchers.IO) {
            if (!state.compareAndSet(NOT_STARTED, RUNNING)) {
                // The cancellation handler below observed the cancellation before we started.
                throw CancellationException()
            }

            try {
                block()
            } catch (e: IOException) {
                // If cancelled, the exception is most likely caused by `close` so report it as
                // cancellation instead.
                ensureActive()
                throw e
            }
        }

        try {
            result.await()
        } catch (e: CancellationException) {
            if (!state.compareAndSet(NOT_STARTED, CANCELLED)) {
                // `block` is running or already ran.
                close()
            }

            throw e
        }
    }
}

// Used for TCP on both jvm and android and for UNIX on jvm.
//
// All operations are cancellable. Cancelling an operation closes the socket.
internal class CommonSocket(private val channel: SocketChannel) : Socket() {
    suspend fun connect(addr: java.net.SocketAddress) {
        runCancellable { channel.connect(addr) }
    }

    override suspend fun read(buffer: ByteBuffer) = runCancellable { channel.read(buffer) }

    override suspend fun write(buffer: ByteBuffer) = runCancellable { channel.write(buffer) }

    override fun close() {
        channel.close()
    }
}

internal expect suspend fun connectSocket(addr: SocketAddress): Socket
