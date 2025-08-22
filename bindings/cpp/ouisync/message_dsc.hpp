#pragma once

#include <ouisync/message_dsc.g.hpp>

namespace ouisync {

template<> struct describe::Struct<ResponseResult::Failure> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;

    template<class Observer>
    static void describe(Observer& o, ResponseResult::Failure& v) {
        o.field(v.code);
        o.field(v.message);
        o.field(v.sources);
    }
};

template<> struct describe::Struct<ResponseResult> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::DIRECT;

    template<class Observer>
    static void describe(Observer& o, ResponseResult& v) {
        o.field(v.value);
    }
};

template<> struct VariantBuilder<ResponseResult::Alternatives> {
    template<class AltBuilder>
    static ResponseResult::Alternatives build(std::string_view name, const AltBuilder& builder) {
        if (name == "Success") {
            return builder.template build<Response>();
        }
        if (name == "Failure") {
            return builder.template build<ResponseResult::Failure>();
        }

        throw std::bad_cast();
    }
};

} // namespace ouisync
