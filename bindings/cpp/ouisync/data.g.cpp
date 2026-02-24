// This file is auto generated. Do not edit.

#include <ouisync/data.hpp>

namespace ouisync {

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
        case error::Service::no_vfs: return "NoVFS";
        case error::Service::other: return "Other";
        default: return "UnknownError";
    }
}

} // namespace ouisync
