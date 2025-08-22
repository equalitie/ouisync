#include <ouisync.hpp>

#include <boost/asio/io_context.hpp>
#include <boost/asio/spawn.hpp>
#include <boost/filesystem/path.hpp>

#include <iostream>

namespace asio = boost::asio;
namespace fs = boost::filesystem;

struct I /* indent */ {
    uint8_t size;
};

namespace std {
    ostream& operator<<(ostream& os, ouisync::EntryType type) {
        switch (type) {
            case ouisync::FILE: return os << "📄";
            case ouisync::DIRECTORY: return os << "📁";
            default: return os;
        }
    }
    ostream& operator<<(ostream& os, I indent) {
        for (auto i = 0; i < indent.size; ++i) os << "  ";
        return os;
    }
} // namespace std

void async_main(asio::yield_context yield) {
    fs::path config_dir =
        fs::path(getenv("HOME")) / ".local/share/org.equalitie.ouisync/configs";
    
    auto session = ouisync::Session::connect(config_dir, yield);
    
    auto repos = session.list_repositories(yield);
    
    std::cout << "Ouisync service contains " << repos.size() << " repositories\n";

    if (repos.empty()) {
        return;
    }

    // List content of the root directory of each repository
    for (auto& [name, repo] : repos) {
        std::cout << I{1} << name << "\n";

        for (auto& entry : repo.read_directory("/", yield)) {
            std::cout << I{2} << entry.entry_type << " " << entry.name << "\n";
        }
    }
}

int main() {
    asio::io_context ctx(1); // TODO: Multi thread support

    asio::spawn(
        ctx,
        async_main,
        [](std::exception_ptr e) {
            if (e) {
                std::rethrow_exception(e);
            }
        }
    );

    try {
        ctx.run();
    }
    catch (const std::exception& e) {
        std::cout << "Error: " << e.what() << "\n";
        return 1;
    }

    return 0;
}
