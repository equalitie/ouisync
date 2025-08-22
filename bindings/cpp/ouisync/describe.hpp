#pragma once

namespace ouisync::describe {

enum class FieldsType {
    // The struct is serialized as zero or one of its fields
    // Example:
    //   struct Foo { std::string foo; };
    //   STR{...}
    DIRECT,
    // The struct is serialized as an array of fields
    // Example:
    //   struct Bar { std::string bar, baz; };
    //   ARRAY{STR{...}, STR{...}}
    ARRAY,
};

template<class E> struct Enum : std::false_type {};
template<class S> struct Struct : std::false_type {};

} // namespace ouisync::describe

#include <ostream>
namespace std {
    ostream& operator<<(ostream& os, ouisync::describe::FieldsType t) {
        switch (t) {
            case ouisync::describe::FieldsType::DIRECT: return os << "DIRECT";
            case ouisync::describe::FieldsType::ARRAY: return os << "ARRAY";
            default: throw std::logic_error("unrecognized FieldType");
        }
    }
}
