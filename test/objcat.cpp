#include "directory.h"
#include "file_blob.h"
#include "block_store.h"
#include "shortcuts.h"
#include "variant.h"
#include "hex.h"

#include <iostream>
#include <boost/filesystem.hpp>
#include <boost/variant.hpp>
#include <boost/serialization/string.hpp>
#include <boost/serialization/vector.hpp>

using namespace std;
using namespace ouisync;

void usage(ostream& os, const string& appname) {
    os << "Usage:\n";
    os << "   " << appname << " -h (print this usage info)\n";
    os << "   " << appname << " <object-file>\n";
}

int main(int argc, char** argv)
{
    string appname = argv[0];

    if (argc < 2) {
        usage(cerr, appname);
        return 1;
    }

    if (string(argv[1]) == "-h") {
        usage(cout, appname);
        return 0;
    }

    fs::path path = argv[1];

    if (!fs::exists(path)) {
        cerr << "File \"" << path << "\" does not exist\n";
        usage(cerr, appname);
        return 2;
    }

    if (!fs::is_regular_file(path)) {
        cerr << "File \"" << path << "\" is not a regular file\n";
        usage(cerr, appname);
        return 3;
    }

    try {
        auto block = BlockStore::load(path);

        Directory dir;
        FileBlob file;

        if (dir.maybe_load(block)) {
            cout << dir << "\n";
        } else if (file.maybe_load(block)) {
            cout << file << "\n";
        } else {
            cout << "Unrecognized block" << "\n";
        }

    } catch (const exception& ex) {
        cerr << ex.what() << "\n";
    }
}
