#include "tagged.h"

#include <sstream>

namespace ouisync::object::tagged::detail {

std::exception bad_tag_exception(Tag requested, Tag parsed)
{
    std::stringstream ss;
    ss << "Object not of the requested type "
       << "(requested:" << requested << "; parsed:" << parsed << ")";
    return std::runtime_error(ss.str());
}

} // namespace
