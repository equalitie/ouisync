#pragma once

#include <boost/system/system_error.hpp>
#include <ouisync/data.hpp>

namespace ouisync::error {

/**
 * Errors originating from the client code
 */
enum Client {
    ok = 0,
    /**
     * Failed to connect to Ouisync service
     */
    connect,
    /**
     * Ouisync service failed to authenticate
     *
     * Likely causes:
     *   * The service is using a different local_endpoint.conf file
     *   * Service is not running and we connected to a socked of some
     *     other aplication
     */
    auth,
    /**
     * Invalid message format
     */
    protocol,
    /**
     * Failed to serialize request
     */
    serialize,
    /**
     * Failed to deserialize response
     */
    deserialize,
    /**
     * Bug in the Ouisync client code
     */
    logic,
};

/**
 * Category of errors originating in the client
 */
class ClientErrorCategory : public boost::system::error_category {
public:
    /**
     * Describe category
     */
    const char* name() const noexcept override {
        return "ClientErrorCategory";
    }

    /**
     * Get error message
     */
    std::string message(int ev) const override {
        switch (static_cast<error::Client>(ev)) {
            case error::connect: return "Failed to connect to Ouisync service";
            case error::auth: return "Failed to authenticate";
            case error::protocol: return "Protocol error";
            case error::serialize: return "Failed to serialize request";
            case error::deserialize: return "Failed to parse response";
            case error::logic: return "Bug in the Ouisync client code";
            default: return "Unknown error";
        }
    }

    /**
     * Map to error condition
     */
    boost::system::error_condition default_error_condition(int ev) const noexcept override {
        // TODO: Map certain errors to a generic condition
        switch (static_cast<error::Client>(ev)) {
            default:
                return boost::system::error_condition(ev, *this);
        }
    }
};

/**
 * Category of errors originating in the Ouisync service
 */
class ServiceErrorCategory : public boost::system::error_category {
public:
    /**
     * Describe category
     */
    const char* name() const noexcept override {
        return "ServiceErrorCategory";
    }

    /**
     * Get error message
     */
    std::string message(int ev) const override {
        return error_message(static_cast<error::Service>(ev));
    }

    /**
     * Map to error condition
     */
    boost::system::error_condition default_error_condition(int ev) const noexcept override {
        // TODO: Map certain errors to a generic condition
        switch (static_cast<error::Service>(ev)) {
            default:
                return boost::system::error_condition(ev, *this);
        }
    }
};

// -------------------------------------
// Boilerplate in ouisync::error namespace
const boost::system::error_category& client_error_category();
const boost::system::error_category& service_error_category();

inline
boost::system::error_code make_error_code(error::Client e) {
    return {static_cast<int>(e), client_error_category()};
}

inline
boost::system::error_code make_error_code(error::Service e) {
    return {static_cast<int>(e), service_error_category()};
}

} // namespace ouisync::error

// -------------------------------------
// Boilerplate in boost::system namespace
namespace boost::system {
    template<> struct is_error_code_enum<ouisync::error::Service>
    {
      static const bool value = true;
    };
} // namespace boost::system

namespace boost::system {
    template<> struct is_error_code_enum<ouisync::error::Client>
    {
      static const bool value = true;
    };
} // namespace boost::system

// -------------------------------------
// Utils
namespace ouisync {

inline void throw_exception(error::Client ec, std::string message) {
    boost::system::system_error e(ec, std::move(message));
    throw e;
}

inline void throw_exception(error::Service ec, std::string message) {
    boost::system::system_error e(ec, std::move(message));
    throw e;
}

} // ouisync namespace

