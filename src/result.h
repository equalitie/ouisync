#pragma once

#include <boost/outcome.hpp>

namespace ouisync {

namespace outcome = BOOST_OUTCOME_V2_NAMESPACE;

template<class T>
using Result = outcome::result<T>;

} // namespace
