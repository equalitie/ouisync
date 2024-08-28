//
//  Generated file. Do not edit.
//

// clang-format off

#include "generated_plugin_registrant.h"

#include <ouisync/ouisync_plugin.h>

void fl_register_plugins(FlPluginRegistry* registry) {
  g_autoptr(FlPluginRegistrar) ouisync_registrar =
      fl_plugin_registry_get_registrar_for_plugin(registry, "OuisyncPlugin");
  ouisync_plugin_register_with_registrar(ouisync_registrar);
}
