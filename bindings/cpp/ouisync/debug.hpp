#pragma once

#include <algorithm>
#include <boost/algorithm/hex.hpp>
#include <iostream>
#include <msgpack.hpp>

namespace ouisync::printer {

    //----------------------------------------------------------------
    template<typename Range> struct Hex {
        const Range& v;
        Hex(const Range& v) : v(v) {}
        friend std::ostream& operator<<(std::ostream& os, const Hex& in) {
            boost::algorithm::hex_lower(in.v, std::ostream_iterator<uint8_t>(os));
            return os;
        }
    };
    template<typename Range>
    Hex<Range> hex(const Range& range) { return Hex<Range>(range); }

    //----------------------------------------------------------------
    template<typename Range> struct Bin {
        const Range& v;
        Bin(const Range& v) : v(v) {}
        friend std::ostream& operator<<(std::ostream& os, const Bin& in) {
            for (auto c : in.v) {
                if (c >= ' ' && c <= '~') {
                    os << c;
                }
                else if (c == 0) {
                    os << "\\0";
                } else {
                    os << "\\x";
                    boost::algorithm::hex(std::string_view(&c, 1), std::ostream_iterator<uint8_t>(os));
                }
            }
            return os;
        }
    };
    template<typename Range>
    Bin<Range> bin(const Range& range) { return Bin<Range>(range); }

    //----------------------------------------------------------------

    struct MsgPackObjType {
        msgpack::type::object_type obj_type;
        MsgPackObjType(msgpack::type::object_type obj_type) : obj_type(obj_type) {}

        friend std::ostream& operator<<(std::ostream& os, const MsgPackObjType& in) {
            using T = msgpack::type::object_type;
            switch (in.obj_type) {
                case T::NIL: return os << "NIL";
                case T::BOOLEAN: return os << "BOOLEAN";
                case T::POSITIVE_INTEGER: return os << "POSITIVE_INTEGER";
                case T::NEGATIVE_INTEGER: return os << "NEGATIVE_INTEGER";
                case T::FLOAT32: return os << "FLOAT32";
                case T::FLOAT64: return os << "FLOAT64";
                case T::STR: return os << "STR";
                case T::BIN: return os << "BIN";
                case T::ARRAY: return os << "ARRAY";
                case T::MAP: return os << "MAP";
                case T::EXT: return os << "EXT";
                default: return os << "???";
            }
        }
    };

    static
    MsgPackObjType msgpack(const msgpack::type::object_type& obj_type) {
        return MsgPackObjType(obj_type);
    }

    //template<class T> struct Cow {
    //    struct Ref { const T& val; };
    //    struct Own { const T  val; };
    //    std::variant<Ref, Own> val;
    //    Cow(const T& val) : val(Ref{val}) {}
    //    Cow(T&& val) : val(Own{std::forward<T>(val)}) {}
    //    T& ref() {
    //        return std::visit([](auto&& val) {
    //            using T = std::decay_t<decltype(val)>;
    //            if constexpr (std::is_same_v<T, Ref>)
    //                return 
    //        }, val);
    //    }
    //};

    struct MsgPack {
        const msgpack::object& obj;
        MsgPack(const msgpack::object& obj) : obj(obj) {}

        friend std::ostream& operator<<(std::ostream& os, const MsgPack& in) {
            using T = msgpack::type::object_type;
            os << msgpack(in.obj.type) << "{";

            switch (in.obj.type) {
                case T::NIL: os << ""; break;
                case T::BOOLEAN: os << in.obj.as<bool>(); break;
                case T::POSITIVE_INTEGER: os << in.obj.as<uint64_t>(); break;
                case T::NEGATIVE_INTEGER: os << in.obj.as<int64_t>(); break;
                case T::FLOAT32: os << in.obj.as<float>(); break;
                case T::FLOAT64: os << in.obj.as<double>(); break;
                case T::STR: os << in.obj.as<std::string_view>(); break;
                case T::BIN: os << "\"" << hex(in.obj.as<std::string_view>()) << "\""; break;
                case T::ARRAY: {
                    auto size = in.obj.via.array.size;
                    for (decltype(size) i = 0; i < size; ++i) {
                        os << MsgPack(in.obj.via.array.ptr[i]);
                        if (i != size - 1) os << ", ";
                    }
                }
                break;
                case T::MAP: {
                    auto size = in.obj.via.map.size;
                    for (decltype(size) i = 0; i < size; ++i) {
                        auto kv = in.obj.via.map.ptr[i];
                        os << "(" << MsgPack(kv.key) << ", " << MsgPack(kv.val) << ")";
                        if (i != size - 1) os << ", ";
                    }
                }
                break;
                case T::EXT: os << "???"; break;
                default: os << "???"; break;
            }

            return os << "}";
        }
    };

    static
    MsgPack display(const msgpack::object& obj) { return MsgPack(obj); }

    //----------------------------------------------------------------
    template<class T> struct Type {
        friend std::ostream& operator<<(std::ostream& os, const Type&) {
            auto name = typeid(T).name();

            int status = -4; // some arbitrary value to eliminate the compiler warning

            // enable c++11 by passing the flag -std=c++11 to g++
            std::unique_ptr<char, void(*)(void*)> res {
                abi::__cxa_demangle(name, NULL, NULL, &status),
                std::free
            };

            return (status==0) ? os << res.get() : os << name ;
        }
    };

    template<class T>
    Type<T> type() { return Type<T>(); }

    //----------------------------------------------------------------

} // namespace ouisync::printer
