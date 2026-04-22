#pragma once

#include <boost/asio/associated_executor.hpp>
#include <boost/asio/buffers_iterator.hpp>
#include <boost/asio/error.hpp>
#include <boost/system/detail/error_code.hpp>
#include <memory>
#include <boost/asio.hpp>
#include <ouisync/api.g.hpp>

namespace ouisync {

/// Adapter for ouisync::File which fullfils the `AsyncReadStream` and `AsyncWriteStream`
/// requirements from asio.
class FileStream {
private:
    struct State {
        ouisync::File file;
        size_t size;
        size_t offset;
    };

public:
    typedef boost::asio::any_io_executor executor_type;

public:

    /// Construct an empty `FileStream` which doesn't wrap any file.
    FileStream() = default;

    FileStream(FileStream&& other) = default;
    FileStream(const FileStream&) = delete;

    FileStream& operator=(FileStream&) = default;
    FileStream& operator=(const FileStream&) = delete;

    ~FileStream();

    executor_type get_executor() {
        return state ? state->file.get_executor() : executor_type{};
    }

    /// Construct a new stream wrapping the given file.
    template<
        boost::asio::completion_token_for<
            void(boost::system::error_code, FileStream)
        > CompletionToken
    >
    static auto init(File file, CompletionToken token) {
        auto state = std::make_shared<State>();
        state->file = std::move(file);

        return boost::asio::async_initiate<CompletionToken, void(boost::system::error_code, FileStream)>(
            [state = std::move(state)]
            (auto handler) mutable
            {
                state->file.get_length(
                    [state, handler = std::move(handler)]
                    (boost::system::error_code ec, size_t size) mutable
                    {
                        if (ec) {
                            return handler(ec, FileStream{});
                        }

                        state->size = size;

                        return handler(
                            boost::system::error_code{},
                            FileStream(std::move(state))
                        );
                    }
                );
            },
            token
        );
    }

    /// Close the wrapped file, leaving the stream empty.
    template<
        boost::asio::completion_token_for<void(boost::system::error_code)> CompletionToken
    >
    void close(CompletionToken token) {
        auto file = take();

        return boost::asio::async_initiate<CompletionToken, void(boost::system::error_code)>(
            [file = std::move(file)](auto handler) mutable {
                if (file) {
                    return file.close(std::move(handler));
                } else {
                    return handler(boost::system::error_code{});
                }
            },
            token
        );
    }

    /// Close without waiting for the operation to complete
    void close() {
        close(boost::asio::detached);
    }

    bool is_open() const {
        return state != nullptr;
    }

    /// Take the file out of this stream, leaving the stream empty.
    ouisync::File take() {
        auto state = std::move(this->state);

        if (state) {
            return std::move(state->file);
        } else {
            return ouisync::File {};
        }
    }

    size_t size() const {
        return state ? state->size : 0;
    }

    /// Returns the current offset of this stream.
    size_t offset() const {
        return state ? state->offset : 0;
    }

    /// Seek to the given position.
    void seek(size_t pos) {
        if (state) {
            state->offset = pos;
        }
    }

    /// Reads data from the file into the provided buffer. Returns the number of bytes actually read.
    template<
        class MutableBufferSequence,
        boost::asio::completion_token_for<
            void(boost::system::error_code, size_t)
        > CompletionToken
    >
    auto async_read_some(const MutableBufferSequence& buffers, CompletionToken token) {
        return boost::asio::async_initiate<CompletionToken, void(boost::system::error_code, size_t)>(
            [ state = state, buffers = std::move(buffers) ]
            (auto handler) mutable {
                size_t n = boost::asio::buffer_size(buffers);

                if (n == 0) {
                    return handler(boost::system::error_code{}, 0);
                }

                if (!state) {
                    return handler(boost::asio::error::not_connected, 0);
                }

                if (state->offset >= state->size) {
                    return handler(boost::asio::error::eof, 0);
                }

                state->file.read(
                    state->offset,
                    n,
                    [ state,
                      handler = std::move(handler),
                      buffers = std::move(buffers) ]
                    (boost::system::error_code ec, std::vector<uint8_t> data) mutable {
                        if (ec) {
                            return handler(ec, data.size());
                        }

                        auto n = boost::asio::buffer_copy(buffers, boost::asio::buffer(data));
                        state->offset += n;

                        return handler(ec, n);
                    }
                );
          },
          token
        );
    }

    /// Writes data from the provided buffers to the file. Returns the number of byte actually
    /// written.
    template<
        class ConstBufferSequence,
        boost::asio::completion_token_for<
            void(boost::system::error_code, size_t)
        > CompletionToken
    >
    auto async_write_some(const ConstBufferSequence& buffers, CompletionToken token) {
        return boost::asio::async_initiate<CompletionToken, void(boost::system::error_code, size_t)>(
            [ state = state, buffers = std::move(buffers) ]
            (auto handler) mutable {
                size_t n = boost::asio::buffer_size(buffers);

                if (n == 0) {
                    return handler({}, 0);
                }

                if (!state) {
                    return handler(boost::asio::error::not_connected, 0);
                }

                std::vector<uint8_t> data(
                    boost::asio::buffers_begin(buffers),
                    boost::asio::buffers_end(buffers)
                );

                state->file.write(
                    state->offset,
                    data,
                    [ state, n, handler = std::move(handler) ]
                    (boost::system::error_code ec) mutable {
                        if (ec) {
                            return handler(ec, 0);
                        }

                        state->offset += n;

                        return handler(ec, n);
                    }
                );
            },
            token
        );
    }

private:

    explicit FileStream(std::shared_ptr<State> state)
        : state(std::move(state))
    {}

    std::shared_ptr<State> state;
};

} // namespace ouisync
