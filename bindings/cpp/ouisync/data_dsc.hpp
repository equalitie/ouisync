#pragma once

#include <ouisync/data_dsc.g.hpp>

namespace ouisync {

template<> struct describe::Struct<MonitorId> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;

    template<class Observer>
    static void describe(Observer& o, MonitorId& v) {
        o.field(v.name);
        o.field(v.disambiguator);
    }
};

template<> struct describe::Struct<StateMonitorNode> : std::true_type {
    static const describe::FieldsType fields_type = describe::FieldsType::ARRAY;

    template<class Observer>
    static void describe(Observer& o, StateMonitorNode& v) {
        o.field(v.values);
        o.field(v.children);
    }
};

} // namespace ouisync


