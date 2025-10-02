#pragma once

namespace ouisync {

// Utility for using std::variant
template<class... Ts>
struct overloaded : Ts... { using Ts::operator()...; };

} // ouisync namespace
