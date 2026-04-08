// This file is auto generated. Do not edit.

#include <ouisync/data.hpp>

namespace ouisync {

std::ostream& operator << (std::ostream& os, const AccessMode& e) {
    switch (e) {
        case AccessMode::blind: os << "blind"; break;
        case AccessMode::read: os << "read"; break;
        case AccessMode::write: os << "write"; break;
    }
    return os;
}

std::string_view variant_name(const LocalSecret::Alternatives& variant) {
    if (std::get_if<LocalSecret::Password>(&variant) != nullptr) {
        return std::string_view("Password");
    }
    if (std::get_if<LocalSecret::SecretKey>(&variant) != nullptr) {
        return std::string_view("SecretKey");
    }
    throw std::bad_cast();
}

std::string_view variant_name(const SetLocalSecret::Alternatives& variant) {
    if (std::get_if<SetLocalSecret::Password>(&variant) != nullptr) {
        return std::string_view("Password");
    }
    if (std::get_if<SetLocalSecret::KeyAndSalt>(&variant) != nullptr) {
        return std::string_view("KeyAndSalt");
    }
    throw std::bad_cast();
}

std::string_view variant_name(const AccessChange::Alternatives& variant) {
    if (std::get_if<AccessChange::Enable>(&variant) != nullptr) {
        return std::string_view("Enable");
    }
    if (std::get_if<AccessChange::Disable>(&variant) != nullptr) {
        return std::string_view("Disable");
    }
    throw std::bad_cast();
}

std::ostream& operator << (std::ostream& os, const EntryType& e) {
    switch (e) {
        case EntryType::file: os << "file"; break;
        case EntryType::directory: os << "directory"; break;
    }
    return os;
}

std::ostream& operator << (std::ostream& os, const NetworkEvent& e) {
    switch (e) {
        case NetworkEvent::protocol_version_mismatch: os << "protocol_version_mismatch"; break;
        case NetworkEvent::peer_set_change: os << "peer_set_change"; break;
    }
    return os;
}

std::ostream& operator << (std::ostream& os, const PeerSource& e) {
    switch (e) {
        case PeerSource::user_provided: os << "user_provided"; break;
        case PeerSource::listener: os << "listener"; break;
        case PeerSource::local_discovery: os << "local_discovery"; break;
        case PeerSource::dht: os << "dht"; break;
        case PeerSource::peer_exchange: os << "peer_exchange"; break;
    }
    return os;
}

std::string_view variant_name(const PeerState::Alternatives& variant) {
    if (std::get_if<PeerState::Known>(&variant) != nullptr) {
        return std::string_view("Known");
    }
    if (std::get_if<PeerState::Connecting>(&variant) != nullptr) {
        return std::string_view("Connecting");
    }
    if (std::get_if<PeerState::Handshaking>(&variant) != nullptr) {
        return std::string_view("Handshaking");
    }
    if (std::get_if<PeerState::Active>(&variant) != nullptr) {
        return std::string_view("Active");
    }
    throw std::bad_cast();
}

std::ostream& operator << (std::ostream& os, const NatBehavior& e) {
    switch (e) {
        case NatBehavior::endpoint_independent: os << "endpoint_independent"; break;
        case NatBehavior::address_dependent: os << "address_dependent"; break;
        case NatBehavior::address_and_port_dependent: os << "address_and_port_dependent"; break;
    }
    return os;
}

namespace error {
std::ostream& operator << (std::ostream& os, const Service& e) {
    switch (e) {
        case Service::ok: os << "ok"; break;
        case Service::permission_denied: os << "permission_denied"; break;
        case Service::invalid_input: os << "invalid_input"; break;
        case Service::invalid_data: os << "invalid_data"; break;
        case Service::already_exists: os << "already_exists"; break;
        case Service::not_found: os << "not_found"; break;
        case Service::ambiguous: os << "ambiguous"; break;
        case Service::unsupported: os << "unsupported"; break;
        case Service::interrupted: os << "interrupted"; break;
        case Service::connection_refused: os << "connection_refused"; break;
        case Service::connection_aborted: os << "connection_aborted"; break;
        case Service::transport_error: os << "transport_error"; break;
        case Service::listener_bind_error: os << "listener_bind_error"; break;
        case Service::listener_accept_error: os << "listener_accept_error"; break;
        case Service::store_error: os << "store_error"; break;
        case Service::is_directory: os << "is_directory"; break;
        case Service::not_directory: os << "not_directory"; break;
        case Service::directory_not_empty: os << "directory_not_empty"; break;
        case Service::resource_busy: os << "resource_busy"; break;
        case Service::runtime_initialize_error: os << "runtime_initialize_error"; break;
        case Service::config_error: os << "config_error"; break;
        case Service::tls_certificates_not_found: os << "tls_certificates_not_found"; break;
        case Service::tls_certificates_invalid: os << "tls_certificates_invalid"; break;
        case Service::tls_keys_not_found: os << "tls_keys_not_found"; break;
        case Service::tls_config_error: os << "tls_config_error"; break;
        case Service::vfs_driver_install_error: os << "vfs_driver_install_error"; break;
        case Service::vfs_other_error: os << "vfs_other_error"; break;
        case Service::service_already_running: os << "service_already_running"; break;
        case Service::store_dir_unspecified: os << "store_dir_unspecified"; break;
        case Service::mount_dir_unspecified: os << "mount_dir_unspecified"; break;
        case Service::other: os << "other"; break;
    }
    return os;
}
} // namespace error

const char* error_message(error::Service ec) noexcept {
    switch (ec) {
        case error::Service::ok: return "Ok";
        case error::Service::permission_denied: return "PermissionDenied";
        case error::Service::invalid_input: return "InvalidInput";
        case error::Service::invalid_data: return "InvalidData";
        case error::Service::already_exists: return "AlreadyExists";
        case error::Service::not_found: return "NotFound";
        case error::Service::ambiguous: return "Ambiguous";
        case error::Service::unsupported: return "Unsupported";
        case error::Service::interrupted: return "Interrupted";
        case error::Service::connection_refused: return "ConnectionRefused";
        case error::Service::connection_aborted: return "ConnectionAborted";
        case error::Service::transport_error: return "TransportError";
        case error::Service::listener_bind_error: return "ListenerBindError";
        case error::Service::listener_accept_error: return "ListenerAcceptError";
        case error::Service::store_error: return "StoreError";
        case error::Service::is_directory: return "IsDirectory";
        case error::Service::not_directory: return "NotDirectory";
        case error::Service::directory_not_empty: return "DirectoryNotEmpty";
        case error::Service::resource_busy: return "ResourceBusy";
        case error::Service::runtime_initialize_error: return "RuntimeInitializeError";
        case error::Service::config_error: return "ConfigError";
        case error::Service::tls_certificates_not_found: return "TlsCertificatesNotFound";
        case error::Service::tls_certificates_invalid: return "TlsCertificatesInvalid";
        case error::Service::tls_keys_not_found: return "TlsKeysNotFound";
        case error::Service::tls_config_error: return "TlsConfigError";
        case error::Service::vfs_driver_install_error: return "VfsDriverInstallError";
        case error::Service::vfs_other_error: return "VfsOtherError";
        case error::Service::service_already_running: return "ServiceAlreadyRunning";
        case error::Service::store_dir_unspecified: return "StoreDirUnspecified";
        case error::Service::mount_dir_unspecified: return "MountDirUnspecified";
        case error::Service::other: return "Other";
        default: return "UnknownError";
    }
}

std::ostream& operator << (std::ostream& os, const LogLevel& e) {
    switch (e) {
        case LogLevel::error: os << "error"; break;
        case LogLevel::warn: os << "warn"; break;
        case LogLevel::info: os << "info"; break;
        case LogLevel::debug: os << "debug"; break;
        case LogLevel::trace: os << "trace"; break;
    }
    return os;
}

} // namespace ouisync
