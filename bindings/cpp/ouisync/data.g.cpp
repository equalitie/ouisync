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
        case error::OK: return "Ok";
        case error::PERMISSION_DENIED: return "PermissionDenied";
        case error::INVALID_INPUT: return "InvalidInput";
        case error::INVALID_DATA: return "InvalidData";
        case error::ALREADY_EXISTS: return "AlreadyExists";
        case error::NOT_FOUND: return "NotFound";
        case error::AMBIGUOUS: return "Ambiguous";
        case error::UNSUPPORTED: return "Unsupported";
        case error::INTERRUPTED: return "Interrupted";
        case error::CONNECTION_REFUSED: return "ConnectionRefused";
        case error::CONNECTION_ABORTED: return "ConnectionAborted";
        case error::TRANSPORT_ERROR: return "TransportError";
        case error::LISTENER_BIND_ERROR: return "ListenerBindError";
        case error::LISTENER_ACCEPT_ERROR: return "ListenerAcceptError";
        case error::STORE_ERROR: return "StoreError";
        case error::IS_DIRECTORY: return "IsDirectory";
        case error::NOT_DIRECTORY: return "NotDirectory";
        case error::DIRECTORY_NOT_EMPTY: return "DirectoryNotEmpty";
        case error::RESOURCE_BUSY: return "ResourceBusy";
        case error::RUNTIME_INITIALIZE_ERROR: return "RuntimeInitializeError";
        case error::CONFIG_ERROR: return "ConfigError";
        case error::TLS_CERTIFICATES_NOT_FOUND: return "TlsCertificatesNotFound";
        case error::TLS_CERTIFICATES_INVALID: return "TlsCertificatesInvalid";
        case error::TLS_KEYS_NOT_FOUND: return "TlsKeysNotFound";
        case error::TLS_CONFIG_ERROR: return "TlsConfigError";
        case error::VFS_DRIVER_INSTALL_ERROR: return "VfsDriverInstallError";
        case error::VFS_OTHER_ERROR: return "VfsOtherError";
        case error::SERVICE_ALREADY_RUNNING: return "ServiceAlreadyRunning";
        case error::STORE_DIR_UNSPECIFIED: return "StoreDirUnspecified";
        case error::MOUNT_DIR_UNSPECIFIED: return "MountDirUnspecified";
        case error::NO_VFS: return "NoVFS";
        case error::OTHER: return "Other";
        default: return "UnknownError";
    }
}

} // namespace ouisync
