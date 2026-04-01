#pragma once

#include <boost/filesystem/operations.hpp>
#include <boost/test/unit_test.hpp>
#include <ouisync/message.hpp>
#include <unordered_map>
#include <unordered_set>

namespace tests::print {
    template<class Collection> struct List {
        const Collection& collection;
    };
    template<class Collection>
    List<Collection> list(const Collection& c) {
        return List<Collection>{c};
    }
} // namespace tests::print

namespace std {
    inline ostream& operator<<(ostream& os, const ouisync::MessageId& m) {
        return os << "MessageId{" << m.value << "}\n";
    }
    template<class T>
    ostream& operator<<(ostream& os, const std::optional<T>& o) {
        if (o) return os << "Some{" << *o << "}";
        else return os << "None";
    }
    template<class Collection>
    ostream& operator<<(ostream& os, const tests::print::List<Collection>& l) {
        for (auto i = l.collection.begin(); i != l.collection.end(); ++i) {
            if (i != l.collection.begin()) os << ", ";
            os << *i;
        }
        return os;
    }
    template<class T1, class T2>
    ostream& operator<<(ostream& os, const std::pair<T1, T2>& p) {
        return os << "(" << p.first << ", " << p.second << ")";
    }
    template<class K, class V>
    ostream& operator<<(ostream& os, const std::unordered_map<K, V>& m) {
        return os << "Map{" << tests::print::list(m) << "}";
    }
    template<class V>
    ostream& operator<<(ostream& os, const std::unordered_set<V>& s) {
        return os << "Set{" << tests::print::list(s) << "}";
    }
} // namespace std

namespace ouisync {
    inline bool operator==(const MessageId& m1, const MessageId& m2) {
        return m1.value == m2.value;
    }
} // ouisync namespace

// Temporary directory that auto-deletes itself on destruction.
class TempDir {
private:
    boost::filesystem::path _path;

public:

    TempDir() :
        _path(
            boost::filesystem::temp_directory_path() /
            "ouisync-cpp-tests" /
            boost::unit_test::framework::current_test_case().p_name /
            boost::filesystem::unique_path()
        )
    {
        boost::filesystem::create_directories(_path);
    }

    ~TempDir() {
        boost::filesystem::remove_all(_path);
    }

    TempDir(const TempDir&) = delete;
    TempDir(TempDir&&) = delete;
    TempDir& operator=(const TempDir&) = delete;
    TempDir& operator=(TempDir&&) = delete;

    const boost::filesystem::path& path() const {
        return _path;
    }

};

void check_exception(std::exception_ptr e);

// Creates a subdirectory at the given path and returns it.
boost::filesystem::path mkdir(const boost::filesystem::path& path);
