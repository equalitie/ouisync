#include <ouisync/error.hpp>

namespace ouisync::error {

const boost::system::error_category& client_error_category() {
    static ClientErrorCategory instance;
    return instance;
}

const boost::system::error_category& service_error_category() {
    static ServiceErrorCategory instance;
    return instance;
}

} // namespace ouisync::error
