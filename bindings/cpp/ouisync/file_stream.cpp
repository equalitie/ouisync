#include <ouisync/file_stream.hpp>

namespace ouisync {

FileStream::~FileStream() {
    close(boost::asio::detached);
}

} // namespace ousiync
