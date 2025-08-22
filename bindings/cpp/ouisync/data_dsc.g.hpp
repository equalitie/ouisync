// This file is auto generated. Do not edit.

#pragma once

#include <ouisync/describe.hpp>

namespace ouisync {

template<typename Variant> struct VariantBuilder;
template<> struct describe::Struct<SecretKey> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, SecretKey& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<Password> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, Password& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<PasswordSalt> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, PasswordSalt& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<StorageSize> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, StorageSize& v) {
        o.field(v.bytes);
    }
};

template<> struct describe::Enum<AccessMode> : std::true_type {
    static bool is_valid(uint8_t v) {
        static const uint8_t values[] = {
            BLIND,
            READ,
            WRITE,
        };
        for (size_t i = 0; i != 3; ++i) {
            if (v == values[i]) return true;
        }
        return false;
    }
};

template<> struct describe::Struct<LocalSecret::Password> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, LocalSecret::Password& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<LocalSecret::SecretKey> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, LocalSecret::SecretKey& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<LocalSecret> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, LocalSecret& v) {
        o.field(v.value);
    }
};

template<> struct VariantBuilder<LocalSecret::Alternatives> {
    template<class AltBuilder>
    static LocalSecret::Alternatives build(std::string_view name, const AltBuilder& builder) {
        if (name == "Password") {
            return builder.template build<LocalSecret::Password>();
        }
        if (name == "SecretKey") {
            return builder.template build<LocalSecret::SecretKey>();
        }

        throw std::runtime_error("invalid variant name for LocalSecret");
    }
};

std::string_view variant_name(const LocalSecret::Alternatives& variant);

template<> struct describe::Struct<SetLocalSecret::Password> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, SetLocalSecret::Password& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<SetLocalSecret::KeyAndSalt> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, SetLocalSecret::KeyAndSalt& v) {
        o.field(v.key);
        o.field(v.salt);
    }
};

template<> struct describe::Struct<SetLocalSecret> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, SetLocalSecret& v) {
        o.field(v.value);
    }
};

template<> struct VariantBuilder<SetLocalSecret::Alternatives> {
    template<class AltBuilder>
    static SetLocalSecret::Alternatives build(std::string_view name, const AltBuilder& builder) {
        if (name == "Password") {
            return builder.template build<SetLocalSecret::Password>();
        }
        if (name == "KeyAndSalt") {
            return builder.template build<SetLocalSecret::KeyAndSalt>();
        }

        throw std::runtime_error("invalid variant name for SetLocalSecret");
    }
};

std::string_view variant_name(const SetLocalSecret::Alternatives& variant);

template<> struct describe::Struct<ShareToken> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, ShareToken& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<AccessChange::Enable> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, AccessChange::Enable& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<AccessChange::Disable> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, AccessChange::Disable& v) {
    }
};

template<> struct describe::Struct<AccessChange> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, AccessChange& v) {
        o.field(v.value);
    }
};

template<> struct VariantBuilder<AccessChange::Alternatives> {
    template<class AltBuilder>
    static AccessChange::Alternatives build(std::string_view name, const AltBuilder& builder) {
        if (name == "Enable") {
            return builder.template build<AccessChange::Enable>();
        }
        if (name == "Disable") {
            return builder.template build<AccessChange::Disable>();
        }

        throw std::runtime_error("invalid variant name for AccessChange");
    }
};

std::string_view variant_name(const AccessChange::Alternatives& variant);

template<> struct describe::Enum<EntryType> : std::true_type {
    static bool is_valid(uint8_t v) {
        static const uint8_t values[] = {
            FILE,
            DIRECTORY,
        };
        for (size_t i = 0; i != 2; ++i) {
            if (v == values[i]) return true;
        }
        return false;
    }
};

template<> struct describe::Enum<NetworkEvent> : std::true_type {
    static bool is_valid(uint8_t v) {
        static const uint8_t values[] = {
            PROTOCOL_VERSION_MISMATCH,
            PEER_SET_CHANGE,
        };
        for (size_t i = 0; i != 2; ++i) {
            if (v == values[i]) return true;
        }
        return false;
    }
};

template<> struct describe::Enum<PeerSource> : std::true_type {
    static bool is_valid(uint8_t v) {
        static const uint8_t values[] = {
            USER_PROVIDED,
            LISTENER,
            LOCAL_DISCOVERY,
            DHT,
            PEER_EXCHANGE,
        };
        for (size_t i = 0; i != 5; ++i) {
            if (v == values[i]) return true;
        }
        return false;
    }
};

template<> struct describe::Struct<PublicRuntimeId> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, PublicRuntimeId& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<PeerState::Known> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, PeerState::Known& v) {
    }
};

template<> struct describe::Struct<PeerState::Connecting> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, PeerState::Connecting& v) {
    }
};

template<> struct describe::Struct<PeerState::Handshaking> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, PeerState::Handshaking& v) {
    }
};

template<> struct describe::Struct<PeerState::Active> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, PeerState::Active& v) {
        o.field(v.id);
        o.field(v.since);
    }
};

template<> struct describe::Struct<PeerState> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, PeerState& v) {
        o.field(v.value);
    }
};

template<> struct VariantBuilder<PeerState::Alternatives> {
    template<class AltBuilder>
    static PeerState::Alternatives build(std::string_view name, const AltBuilder& builder) {
        if (name == "Known") {
            return builder.template build<PeerState::Known>();
        }
        if (name == "Connecting") {
            return builder.template build<PeerState::Connecting>();
        }
        if (name == "Handshaking") {
            return builder.template build<PeerState::Handshaking>();
        }
        if (name == "Active") {
            return builder.template build<PeerState::Active>();
        }

        throw std::runtime_error("invalid variant name for PeerState");
    }
};

std::string_view variant_name(const PeerState::Alternatives& variant);

template<> struct describe::Struct<Stats> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Stats& v) {
        o.field(v.bytes_tx);
        o.field(v.bytes_rx);
        o.field(v.throughput_tx);
        o.field(v.throughput_rx);
    }
};

template<> struct describe::Struct<PeerInfo> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, PeerInfo& v) {
        o.field(v.addr);
        o.field(v.source);
        o.field(v.state);
        o.field(v.stats);
    }
};

template<> struct describe::Struct<Progress> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, Progress& v) {
        o.field(v.value);
        o.field(v.total);
    }
};

template<> struct describe::Enum<NatBehavior> : std::true_type {
    static bool is_valid(uint8_t v) {
        static const uint8_t values[] = {
            ENDPOINT_INDEPENDENT,
            ADDRESS_DEPENDENT,
            ADDRESS_AND_PORT_DEPENDENT,
        };
        for (size_t i = 0; i != 3; ++i) {
            if (v == values[i]) return true;
        }
        return false;
    }
};

template<> struct describe::Enum<error::Service> : std::true_type {
    static bool is_valid(uint16_t v) {
        static const uint16_t values[] = {
            error::OK,
            error::PERMISSION_DENIED,
            error::INVALID_INPUT,
            error::INVALID_DATA,
            error::ALREADY_EXISTS,
            error::NOT_FOUND,
            error::AMBIGUOUS,
            error::UNSUPPORTED,
            error::INTERRUPTED,
            error::CONNECTION_REFUSED,
            error::CONNECTION_ABORTED,
            error::TRANSPORT_ERROR,
            error::LISTENER_BIND_ERROR,
            error::LISTENER_ACCEPT_ERROR,
            error::STORE_ERROR,
            error::IS_DIRECTORY,
            error::NOT_DIRECTORY,
            error::DIRECTORY_NOT_EMPTY,
            error::RESOURCE_BUSY,
            error::RUNTIME_INITIALIZE_ERROR,
            error::CONFIG_ERROR,
            error::TLS_CERTIFICATES_NOT_FOUND,
            error::TLS_CERTIFICATES_INVALID,
            error::TLS_KEYS_NOT_FOUND,
            error::TLS_CONFIG_ERROR,
            error::VFS_DRIVER_INSTALL_ERROR,
            error::VFS_OTHER_ERROR,
            error::SERVICE_ALREADY_RUNNING,
            error::STORE_DIR_UNSPECIFIED,
            error::MOUNT_DIR_UNSPECIFIED,
            error::OTHER,
        };
        for (size_t i = 0; i != 31; ++i) {
            if (v == values[i]) return true;
        }
        return false;
    }
};

template<> struct describe::Enum<LogLevel> : std::true_type {
    static bool is_valid(uint8_t v) {
        static const uint8_t values[] = {
            ERROR,
            WARN,
            INFO,
            DEBUG,
            TRACE,
        };
        for (size_t i = 0; i != 5; ++i) {
            if (v == values[i]) return true;
        }
        return false;
    }
};

template<> struct describe::Struct<MessageId> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, MessageId& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<MetadataEdit> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, MetadataEdit& v) {
        o.field(v.key);
        o.field(v.old_value);
        o.field(v.new_value);
    }
};

template<> struct describe::Struct<NetworkDefaults> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, NetworkDefaults& v) {
        o.field(v.bind);
        o.field(v.port_forwarding_enabled);
        o.field(v.local_discovery_enabled);
    }
};

template<> struct describe::Struct<DirectoryEntry> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, DirectoryEntry& v) {
        o.field(v.name);
        o.field(v.entry_type);
    }
};

template<> struct describe::Struct<QuotaInfo> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;
    template<class Observer>
    static void describe(Observer& o, QuotaInfo& v) {
        o.field(v.quota);
        o.field(v.size);
    }
};

template<> struct describe::Struct<FileHandle> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, FileHandle& v) {
        o.field(v.value);
    }
};

template<> struct describe::Struct<RepositoryHandle> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;
    template<class Observer>
    static void describe(Observer& o, RepositoryHandle& v) {
        o.field(v.value);
    }
};

} // namespace ouisync
