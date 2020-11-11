#include "options.h"

using namespace ouisync;
using namespace boost::program_options;

Options::Options() :
    help(false),
    description("Options")
{
    description.add_options()
        ( "help,h", value(&help), "produce this help")

        ( "basedir", value(&basedir)->required(), "base directory")

        ;
}

void Options::parse(unsigned argc, char** argv)
{
    variables_map vars;
    store(parse_command_line(argc, argv, description), vars);
    notify(vars);

    branchdir         = basedir / "branches";
    objectdir         = basedir / "objects";
    mountdir          = basedir / "mount";
    user_id_file_path = basedir / "user_id";
}

void Options::write_help(std::ostream& os)
{
    os << description;
}
