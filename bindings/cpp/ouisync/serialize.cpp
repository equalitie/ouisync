#include <ouisync/serialize.hpp>
#include <ouisync/data_dsc.hpp>
#include <ouisync/message_dsc.hpp>
#include <ouisync/error.hpp>

#include <msgpack.hpp>
#include <chrono>

//
// C++ concepts
//
namespace ouisync {

// Concept which tells whether a struct is described
template<class T> concept is_described_struct = describe::Struct<std::decay_t<T>>::value;

// Concept which tells whether an enum is described
template<class T> concept is_described_enum = describe::Enum<T>::value;

// Observers for described structs
struct Description {
    const describe::FieldsType type;
    size_t count;
    friend std::ostream& operator<<(std::ostream& os, Description d) {
        switch (d.type) {
            case describe::FieldsType::ARRAY: os << "ARRAY"; break;
            case describe::FieldsType::DIRECT: os << "DIRECT"; break;
            default: os << "???";
        }
        return os << ":" << d.count;
    }
};

struct DescriptionObserver {
    Description dsc;
    DescriptionObserver(describe::FieldsType type) : dsc(Description{type, 0}) {}
    template<class M> void field(M&) { ++dsc.count; }
};

template<is_described_struct S>
Description description(S& s) {
    using T = std::decay_t<S>;
    using Describe = describe::Struct<T>;
    DescriptionObserver observer(Describe::fields_type);
    Describe::describe(observer, const_cast<T&>(s));
    return observer.dsc;
}

template<class Packer>
struct PackObserver {
    Packer& pk;
    bool prepared_array = false;
    const Description dsc;

    PackObserver(Packer& pk, Description dsc)
        : pk(pk), dsc(dsc) {}

    template<class M> void field(M& field_in) {
        if (dsc.type == describe::FieldsType::DIRECT) {
            pk.pack(field_in);
            return;
        }

        if (!prepared_array) {
            pk.pack_array(dsc.count);
            prepared_array = true;
        }

        pk.pack(field_in);
    }
};

template<class Packer, is_described_struct In>
void describe_pack(Packer& packer, const In& in) {
    using Describe = describe::Struct<In>;
    PackObserver pack_observer(packer, description(in));
    // Const cast is OK as long as PackObserver doesn't modify `in`.
    Describe::describe(pack_observer, const_cast<In&>(in));
}

struct UnpackObserver {
    const Description dsc;
    const msgpack::object& obj;
    bool array_checked = false;
    size_t parsed = 0;

    UnpackObserver(const msgpack::object& obj, Description dsc)
        : dsc(dsc), obj(obj) {}

    template<class M> void field(M& member_out) {
        if (dsc.type == describe::FieldsType::DIRECT) {
            if (dsc.count == 0) {
                throw_exception(error::deserialize, "wrong description");
            }
            if (parsed > 0) {
                throw_exception(error::deserialize, "wrong description");
            }
            member_out = obj.as<M>();
            ++parsed;
        }
        else if (dsc.type == describe::FieldsType::ARRAY) {
            if (!array_checked) {
                if (obj.via.array.size != dsc.count) {
                    throw_exception(error::deserialize, "wrong description");
                }
                array_checked = true;
            }
            member_out = obj.via.array.ptr[parsed++].as<M>();
        }
        else {
            throw_exception(error::logic, "unreachable");
        }
    }
};

template<is_described_struct Out>
void describe_unpack(const msgpack::object& obj, Out& out) {
    using T = std::decay_t<Out>;
    using Describe = describe::Struct<T>;
    UnpackObserver unpack_observer(obj, description(out));
    Describe::describe(unpack_observer, out);
}

} // namespace ouisync

namespace msgpack {
MSGPACK_API_VERSION_NAMESPACE(MSGPACK_DEFAULT_API_NS) {
namespace adaptor {

//
// Serialize std::chrono::milliseconds
//
template<>
struct convert<std::chrono::milliseconds> {
    msgpack::object const& operator()(msgpack::object const& obj, std::chrono::milliseconds& out) const {
        uint64_t millis = obj.as<uint64_t>();
        out = std::chrono::milliseconds(millis);
        return obj;
    }
};

template<>
struct pack<std::chrono::milliseconds> {
    template <typename Stream>
    packer<Stream>& operator()(msgpack::packer<Stream>& pk, std::chrono::milliseconds const& millis) const {
        pk.pack(static_cast<uint64_t>(millis.count()));
        return pk;
    }
};

//
// Serialize described enums
//
template<ouisync::is_described_enum E>
struct convert<E> {
    msgpack::object const& operator()(msgpack::object const& obj, E& out) const {
        using T = std::underlying_type_t<E>;

        T n = obj.as<T>();

        if (!ouisync::describe::Enum<E>::is_valid(n)) {
            ouisync::throw_exception(ouisync::error::deserialize, "Invalid variant");
        }

        out = static_cast<E>(n);

        return obj;
    }
};

template<ouisync::is_described_enum E>
struct pack<E> {
    template <typename Stream>
    packer<Stream>& operator()(msgpack::packer<Stream>& pk, E const& v) const {
        pk.pack(static_cast<std::underlying_type_t<E>>(v));
        return pk;
    }
};

//
// Serialize described structs
//
template<ouisync::is_described_struct T>
struct convert<T> {
    msgpack::object const& operator()(msgpack::object const& obj, T& out) const {
        ouisync::describe_unpack(obj, out);
        return obj;
    }
};

template<ouisync::is_described_struct T>
struct pack<T> {
    template <typename Stream>
    packer<Stream>& operator()(msgpack::packer<Stream>& pk, T const& in) const {
        ouisync::describe_pack(pk, in);
        return pk;
    }
};

//
// Serialize std::variant (called complex enums in the generator)
//
template<class Variant>
struct Builder {
    msgpack::object obj;

    template<class Kind> Variant build() const {
        return obj.as<Kind>();
    }
};

template<class... Ts>
struct convert<std::variant<Ts...>> {
    msgpack::object const& operator()(msgpack::object const& obj, std::variant<Ts...>& out) const {
        if (obj.type != msgpack::type::MAP) {
            ouisync::throw_exception(ouisync::error::deserialize, "variant not packed as map");
        }

        if (obj.via.map.size != 1) {
            ouisync::throw_exception(ouisync::error::deserialize, "variant map size is not 1");
        }

        auto kv = obj.via.map.ptr[0];
        auto name = kv.key.as<std::string_view>();

        using Out = std::decay_t<decltype(out)>;
        out = ouisync::VariantBuilder<Out>::build(name, Builder<Out>(kv.val));

        return obj;
    }
};

template<class... Ts>
struct pack<std::variant<Ts...>> {
    template <typename Stream>
    packer<Stream>& operator()(msgpack::packer<Stream>& pk, std::variant<Ts...> const& in) const {
        std::visit(
            [&pk](auto&& alt) {
                auto dsc = description(alt);

                if (dsc.count == 0) {
                    pk.pack(ouisync::variant_name(alt));
                } else {
                    pk.pack_map(1);
                    pk.pack(ouisync::variant_name(alt));
                    pk.pack(alt);
                }
            },
            in);

        return pk;
    }
};

} // adaptor
} // MSGPACK_API_VERSION_NAMESPACE(MSGPACK_DEFAULT_API_NS)
} // msgpack

namespace ouisync {

std::stringstream serialize(const Request& request) {
    std::stringstream buffer;
    msgpack::pack(buffer, request);

    // Debug
    //{
    //    msgpack::unpacked msgpack_result;
    //    msgpack::unpack(msgpack_result, buffer.view().data(), buffer.view().size());
    //    msgpack::object obj = msgpack_result.get();
    //    std::cout << ">>> " << printer::display(obj) << "\n";
    //}

    return buffer;
}

ResponseResult deserialize(const std::vector<char>& buffer) {
    msgpack::unpacked msgpack_result;
    msgpack::unpack(msgpack_result, buffer.data(), buffer.size()); 
    msgpack::object obj = msgpack_result.get();

    // Debug
    //std::cout << "<<<d " << printer::display(obj) << "\n";

    return obj.as<ResponseResult>();
}

} // namespace ouisync
